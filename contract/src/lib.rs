//! Leash vault: an on-chain spending policy for an AI agent.
//!
//! The agent holds a function-call key that can only call `request_spend` on
//! this contract. Every rule below runs inside the contract, so a jailbroken or
//! prompt-injected agent still cannot move money outside the policy:
//!
//! 1. Frozen vault            -> blocked
//! 2. Receiver on blocklist   -> blocked
//! 3. Not enough free balance -> blocked
//! 4. Too many open requests  -> blocked
//! 5. Over the per-payment cap, over the daily cap, over that receiver's own
//!    daily limit, or a receiver that is not on the allowlist
//!                            -> pending, until the owner approves or denies
//! 6. Otherwise               -> paid immediately
//!
//! Only the owner can approve, deny, freeze, or change the policy.
//! `check_spend` runs the same rules as a free view call, so an agent (or a
//! person) can ask what would happen before signing anything.
//!
//! Tripwire: if the contract blocks too many requests in a short window, the
//! vault freezes itself. An agent that keeps attacking is cut off without
//! waiting for the owner to notice.
//!
//! Cross-chain (Chain Signatures): with `chain: "ethereum"`, the same method
//! and the same rules govern an Ethereum address that NEAR's MPC network
//! derives for this contract. The vault builds the EIP-1559 transaction
//! itself, hashes it, and only then asks the MPC signer (`v1.signer-prod.testnet`)
//! to sign. Nobody else can request a signature under the vault's key, so the
//! policy is the only way that Ethereum money moves.

use near_sdk::json_types::U128;
use near_sdk::store::{IterableMap, IterableSet, LookupMap, Vector};
use near_sdk::{
    env, log, near, require, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
};

const DAY_NS: u64 = 24 * 60 * 60 * 1_000_000_000;
const MAX_REASON_LEN: usize = 280;
const MAX_OPEN_PENDING: u32 = 25;
/// Kept aside so the vault can always pay for its own storage.
const RESERVE: NearToken = NearToken::from_millinear(500);
const CALLBACK_GAS: Gas = Gas::from_tgas(5);
/// Derivation path of the vault's Ethereum key on the MPC signer.
pub const ETH_PATH: &str = "leash-eth";
/// A plain ETH transfer always costs 21,000 gas.
const ETH_GAS_LIMIT: u64 = 21_000;
const MPC_SIGN_GAS: Gas = Gas::from_tgas(50);
const ON_SIGNED_GAS: Gas = Gas::from_tgas(10);
const MAX_SIGNATURE_LEN: usize = 1024;

#[near(serializers = [borsh])]
#[derive(BorshStorageKey)]
enum Prefix {
    Allowlist,
    Blocklist,
    Requests,
    ReceiverCaps,
    ReceiverSpent,
    EthTxs,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Paid automatically because it was inside the policy.
    Executed,
    /// Waiting for the owner.
    Pending,
    /// Approved by the owner and paid.
    Approved,
    /// Rejected by the owner.
    Denied,
    /// Refused by a hard rule. Never paid.
    Blocked,
    /// The transfer was sent but bounced (for example, the receiver does not exist),
    /// or the MPC network did not return a signature.
    Failed,
    /// Cross-chain only: inside the policy (or approved), waiting for the MPC
    /// signature. Becomes Executed / Approved once signed.
    Signing,
}

/// Policy for the vault's Ethereum address, in wei. One EVM chain per vault.
#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq)]
pub struct EthPolicy {
    /// 11155111 for Sepolia.
    pub chain_id: u64,
    /// The Chain Signatures MPC contract, `v1.signer-prod.testnet` on testnet.
    pub mpc_contract: AccountId,
    pub per_tx_cap: U128,
    pub daily_cap: U128,
    /// Fee ceiling written into every transaction. Each transfer can cost at
    /// most 21,000 x max_fee_per_gas, so the agent cannot burn money on gas.
    pub max_fee_per_gas: U128,
    pub max_priority_fee_per_gas: U128,
    /// Nonce for the next transaction the vault signs.
    pub next_nonce: u64,
}

/// The MPC network's ECDSA signature, as it returns it.
#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq)]
pub struct EthSignature {
    /// Compressed R point, hex. r is its x coordinate (bytes 1..33).
    pub big_r: String,
    pub s: String,
    pub recovery_id: u8,
}

/// The Ethereum transaction behind a cross-chain request. Built by the vault,
/// so the signed bytes always match the policy decision. Anyone can broadcast
/// it once `signature` is set.
#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq)]
pub struct EthTx {
    pub chain_id: u64,
    pub to: String,
    pub value: U128,
    pub gas_limit: u64,
    pub max_fee_per_gas: U128,
    pub max_priority_fee_per_gas: U128,
    /// Set when signing starts (right away, or when the owner approves).
    pub nonce: Option<u64>,
    /// keccak256 of the unsigned EIP-1559 transaction, hex, sent to the MPC signer.
    pub payload: Option<String>,
    pub signature: Option<EthSignature>,
}

/// A request as the views return it: the stored request, plus the Ethereum
/// transaction when it is a cross-chain one.
#[near(serializers = [json])]
pub struct RequestView {
    #[serde(flatten)]
    pub request: Request,
    /// "near" or "ethereum".
    pub chain: String,
    pub eth: Option<EthTx>,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    pub receiver: AccountId,
    pub amount: U128,
    /// What the agent says the payment is for.
    pub reason: String,
    pub status: Status,
    /// Machine-readable rule that decided the outcome, e.g. `over_per_tx_cap`.
    pub rule: String,
    pub created_at: u64,
    pub resolved_at: Option<u64>,
}

/// Freeze the vault after `max_blocked` blocked requests within `window_secs`.
/// `cooldown_secs: None` keeps it frozen until the owner unfreezes it.
#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq)]
pub struct Tripwire {
    pub max_blocked: u32,
    pub window_secs: u64,
    pub cooldown_secs: Option<u64>,
}

/// Running totals since the vault was created, kept on-chain.
#[near(serializers = [borsh])]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    requests: u64,
    paid: u64,
    near_paid: u128,
    held: u64,
    denied: u64,
    near_denied: u128,
    blocked: u64,
    near_blocked: u128,
    failed: u64,
    tripwire_trips: u64,
}

#[near(serializers = [json])]
pub struct StatsView {
    pub requests: u64,
    /// Paid inside the policy or approved by the owner.
    pub paid: u64,
    pub near_paid: U128,
    /// Ever sent to the owner for a decision.
    pub held: u64,
    pub denied: u64,
    pub near_denied: U128,
    pub blocked: u64,
    pub near_blocked: U128,
    pub failed: u64,
    pub tripwire_trips: u64,
}

/// What `check_spend` says the vault would do with a request right now.
#[near(serializers = [json])]
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub status: Status,
    pub rule: String,
}

/// A receiver-specific daily limit and what that receiver got in the current window.
#[near(serializers = [json])]
pub struct ReceiverCapView {
    pub receiver: AccountId,
    pub daily_cap: U128,
    pub spent_today: U128,
}

#[near(serializers = [json])]
pub struct VaultView {
    pub owner: AccountId,
    pub agent: AccountId,
    pub per_tx_cap: U128,
    pub daily_cap: U128,
    pub spent_today: U128,
    pub window_started_at: u64,
    pub frozen: bool,
    pub balance: U128,
    pub available: U128,
    pub allowlist: Vec<AccountId>,
    pub blocklist: Vec<(AccountId, String)>,
    pub open_pending: u32,
    pub total_requests: u64,
    pub receiver_caps: Vec<ReceiverCapView>,
    /// True only for a manual freeze; `frozen` is also true while the tripwire holds.
    pub frozen_by_owner: bool,
    /// Set while the tripwire holds the vault. u64::MAX means until the owner unfreezes.
    pub tripped_until: Option<u64>,
    pub tripwire: Option<Tripwire>,
    /// Blocked requests counted toward the tripwire in the current window.
    pub recent_blocked: u32,
    pub stats: StatsView,
    /// Cross-chain policy, if enabled. Path on the MPC signer: `leash-eth`.
    pub eth: Option<EthPolicy>,
    pub eth_spent_today: U128,
    /// Wei signed inside the policy or approved, and wei refused (blocked or denied).
    pub wei_paid: U128,
    pub wei_refused: U128,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Leash {
    owner: AccountId,
    agent: AccountId,
    per_tx_cap: NearToken,
    daily_cap: NearToken,
    spent_in_window: NearToken,
    window_started_at: u64,
    frozen: bool,
    open_pending: u32,
    allowlist: IterableSet<AccountId>,
    blocklist: IterableMap<AccountId, String>,
    requests: Vector<Request>,
    /// Optional per-receiver daily limits, on top of the vault-wide daily cap.
    receiver_caps: IterableMap<AccountId, NearToken>,
    /// (window_started_at, spent) per receiver. A stale window counts as zero,
    /// so the 24 h roll and `reset_window` clear these without iterating.
    receiver_spent: LookupMap<AccountId, (u64, NearToken)>,
    tripwire: Option<Tripwire>,
    tripped_until: Option<u64>,
    /// Timestamps of recent blocked requests, at most `tripwire.max_blocked`.
    recent_blocked: Vec<u64>,
    stats: Stats,
    eth: Option<EthPolicy>,
    /// Wei signed in the current daily window (same window as the NEAR cap).
    eth_spent_in_window: u128,
    eth_txs: LookupMap<u64, EthTx>,
    wei_paid: u128,
    wei_refused: u128,
}

/// State layout of v1.2.0 (tripwire and stats, no cross-chain), kept so
/// `migrate` can read the vault that is deployed today.
#[near(serializers = [borsh])]
pub struct LeashV12 {
    owner: AccountId,
    agent: AccountId,
    per_tx_cap: NearToken,
    daily_cap: NearToken,
    spent_in_window: NearToken,
    window_started_at: u64,
    frozen: bool,
    open_pending: u32,
    allowlist: IterableSet<AccountId>,
    blocklist: IterableMap<AccountId, String>,
    requests: Vector<Request>,
    receiver_caps: IterableMap<AccountId, NearToken>,
    receiver_spent: LookupMap<AccountId, (u64, NearToken)>,
    tripwire: Option<Tripwire>,
    tripped_until: Option<u64>,
    recent_blocked: Vec<u64>,
    stats: Stats,
}

const MAX_TRIPWIRE_COUNT: u32 = 20;
const MAX_TRIPWIRE_SECS: u64 = 30 * 24 * 60 * 60;

#[near]
impl Leash {
    #[init]
    pub fn new(owner: AccountId, agent: AccountId, per_tx_cap: U128, daily_cap: U128) -> Self {
        Self {
            owner,
            agent,
            per_tx_cap: NearToken::from_yoctonear(per_tx_cap.0),
            daily_cap: NearToken::from_yoctonear(daily_cap.0),
            spent_in_window: NearToken::from_yoctonear(0),
            window_started_at: env::block_timestamp(),
            frozen: false,
            open_pending: 0,
            allowlist: IterableSet::new(Prefix::Allowlist),
            blocklist: IterableMap::new(Prefix::Blocklist),
            requests: Vector::new(Prefix::Requests),
            receiver_caps: IterableMap::new(Prefix::ReceiverCaps),
            receiver_spent: LookupMap::new(Prefix::ReceiverSpent),
            tripwire: None,
            tripped_until: None,
            recent_blocked: Vec::new(),
            stats: Stats::default(),
            eth: None,
            eth_spent_in_window: 0,
            eth_txs: LookupMap::new(Prefix::EthTxs),
            wei_paid: 0,
            wei_refused: 0,
        }
    }

    /// One-time upgrade from the v1.2.0 layout. Only the vault account itself
    /// can call it (`npm run owner -- upgrade migrate` deploys and migrates in
    /// one transaction). Everything is kept; cross-chain starts disabled.
    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        let old: LeashV12 = env::state_read().unwrap_or_else(|| env::panic_str("no_v1_2_state"));
        Self {
            owner: old.owner,
            agent: old.agent,
            per_tx_cap: old.per_tx_cap,
            daily_cap: old.daily_cap,
            spent_in_window: old.spent_in_window,
            window_started_at: old.window_started_at,
            frozen: old.frozen,
            open_pending: old.open_pending,
            allowlist: old.allowlist,
            blocklist: old.blocklist,
            requests: old.requests,
            receiver_caps: old.receiver_caps,
            receiver_spent: old.receiver_spent,
            tripwire: old.tripwire,
            tripped_until: old.tripped_until,
            recent_blocked: old.recent_blocked,
            stats: old.stats,
            eth: None,
            eth_spent_in_window: 0,
            eth_txs: LookupMap::new(Prefix::EthTxs),
            wei_paid: 0,
            wei_refused: 0,
        }
    }

    // ------------------------------------------------------------------
    // Agent
    // ------------------------------------------------------------------

    /// The only method the agent's key can call. Never panics on a policy
    /// violation: the refusal is recorded on-chain so it can be audited.
    ///
    /// `chain`: omit (or "near") to pay NEAR to a NEAR account. "ethereum"
    /// pays `amount` wei to `receiver`, a lowercase 0x address, from the
    /// vault's Chain Signatures address, under the vault's Ethereum caps.
    pub fn request_spend(
        &mut self,
        receiver: AccountId,
        amount: U128,
        reason: String,
        chain: Option<String>,
    ) -> Request {
        require!(env::predecessor_account_id() == self.agent, "only_agent");
        require!(amount.0 > 0, "amount_must_be_positive");
        let eth = parse_chain(chain.as_deref(), &receiver);
        let reason: String = reason.chars().take(MAX_REASON_LEN).collect();
        self.roll_window();

        let Decision { status, mut rule } = self.decide(&receiver, amount.0, eth);
        if status == Status::Blocked && !self.is_frozen() && self.note_block_and_maybe_trip() {
            rule = format!("{rule}; tripwire_tripped");
        }
        self.stats.requests += 1;
        match status {
            Status::Executed => {
                self.stats.paid += 1;
                if eth {
                    self.wei_paid += amount.0;
                } else {
                    self.stats.near_paid += amount.0;
                }
            }
            Status::Pending => self.stats.held += 1,
            _ => {
                self.stats.blocked += 1;
                if eth {
                    self.wei_refused += amount.0;
                } else {
                    self.stats.near_blocked += amount.0;
                }
            }
        }

        let id = self.requests.len() as u64;
        let mut request = Request {
            id,
            receiver: receiver.clone(),
            amount,
            reason,
            status: status.clone(),
            rule,
            created_at: env::block_timestamp(),
            resolved_at: if status == Status::Pending {
                None
            } else {
                Some(env::block_timestamp())
            },
        };
        if eth {
            // Kept for blocked requests too, so the ledger shows them as
            // Ethereum. Only an allowed request ever gets a nonce and a hash.
            let (chain_id, max_fee, tip) = self
                .eth
                .as_ref()
                .map_or((0, U128(0), U128(0)), |p| (p.chain_id, p.max_fee_per_gas, p.max_priority_fee_per_gas));
            self.eth_txs.insert(
                id,
                EthTx {
                    chain_id,
                    to: receiver.to_string(),
                    value: amount,
                    gas_limit: ETH_GAS_LIMIT,
                    max_fee_per_gas: max_fee,
                    max_priority_fee_per_gas: tip,
                    nonce: None,
                    payload: None,
                    signature: None,
                },
            );
        }
        if eth && status == Status::Executed {
            // Paid in the sense of "allowed": it becomes Executed once signed.
            request.status = Status::Signing;
            request.resolved_at = None;
        }
        self.requests.push(request.clone());

        match status {
            Status::Executed if eth => {
                self.record_eth_spend(&receiver, amount.0);
                self.start_signing(id);
                emit("spend_signing", &request);
            }
            Status::Executed => {
                let amount_t = NearToken::from_yoctonear(amount.0);
                self.record_spend(&receiver, amount_t);
                self.pay(id, &receiver, amount_t);
                emit("spend_executed", &request);
            }
            Status::Pending => {
                self.open_pending += 1;
                emit("spend_pending", &request);
            }
            _ => emit("spend_blocked", &request),
        }
        request
    }

    // ------------------------------------------------------------------
    // Owner
    // ------------------------------------------------------------------

    pub fn approve(&mut self, id: u64) -> Request {
        self.assert_owner();
        let mut r = self.take_pending(id);
        require!(!self.is_frozen(), "vault_frozen");
        self.stats.paid += 1;
        r.rule = format!("{} -> approved_by_owner", r.rule);
        self.roll_window();
        if self.eth_txs.contains_key(&id) {
            require!(self.eth.is_some(), "chain_not_enabled");
            self.wei_paid += r.amount.0;
            r.status = Status::Signing;
            self.record_eth_spend(&r.receiver, r.amount.0);
            self.requests.set(id as u32, r.clone());
            self.start_signing(id);
            emit("spend_signing", &r);
            return r;
        }
        let amount = NearToken::from_yoctonear(r.amount.0);
        require!(amount <= self.available(), "insufficient_funds");
        self.stats.near_paid += r.amount.0;
        r.status = Status::Approved;
        r.resolved_at = Some(env::block_timestamp());
        self.record_spend(&r.receiver, amount);
        self.pay(id, &r.receiver, amount);
        self.requests.set(id as u32, r.clone());
        emit("spend_approved", &r);
        r
    }

    pub fn deny(&mut self, id: u64) -> Request {
        self.assert_owner();
        let mut r = self.take_pending(id);
        r.status = Status::Denied;
        r.rule = format!("{} -> denied_by_owner", r.rule);
        self.stats.denied += 1;
        if self.eth_txs.contains_key(&id) {
            self.wei_refused += r.amount.0;
        } else {
            self.stats.near_denied += r.amount.0;
        }
        r.resolved_at = Some(env::block_timestamp());
        self.requests.set(id as u32, r.clone());
        emit("spend_denied", &r);
        r
    }

    /// Kill switch: every agent request is blocked until `unfreeze`.
    pub fn freeze(&mut self) {
        self.assert_owner();
        self.frozen = true;
        log!("EVENT_JSON:{{\"standard\":\"leash\",\"version\":\"1.0.0\",\"event\":\"frozen\",\"data\":[]}}");
    }

    /// Lifts both a manual freeze and a tripwire freeze.
    pub fn unfreeze(&mut self) {
        self.assert_owner();
        self.frozen = false;
        self.tripped_until = None;
        self.recent_blocked.clear();
        log!("EVENT_JSON:{{\"standard\":\"leash\",\"version\":\"1.0.0\",\"event\":\"unfrozen\",\"data\":[]}}");
    }

    /// Starts a fresh daily window now. Owner only. The hosted demo calls this
    /// with a function-call key scoped to this one method, so visitors share
    /// one vault without exhausting each other's daily allowance.
    pub fn reset_window(&mut self) {
        self.assert_owner();
        self.window_started_at = env::block_timestamp();
        self.spent_in_window = NearToken::from_yoctonear(0);
        self.eth_spent_in_window = 0;
        log!("EVENT_JSON:{{\"standard\":\"leash\",\"version\":\"1.0.0\",\"event\":\"window_reset\",\"data\":[]}}");
    }

    /// Turn on (or update) cross-chain spending through Chain Signatures.
    /// Amounts and fees are in wei. `next_nonce` must match the vault's
    /// Ethereum address (0 for a fresh one); call again to resync it.
    #[allow(clippy::too_many_arguments)]
    pub fn enable_eth(
        &mut self,
        chain_id: u64,
        mpc_contract: AccountId,
        per_tx_cap: U128,
        daily_cap: U128,
        max_fee_per_gas: U128,
        max_priority_fee_per_gas: U128,
        next_nonce: u64,
    ) {
        self.assert_owner();
        require!(max_priority_fee_per_gas.0 <= max_fee_per_gas.0, "priority_fee_above_max_fee");
        self.eth = Some(EthPolicy {
            chain_id,
            mpc_contract,
            per_tx_cap,
            daily_cap,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            next_nonce,
        });
    }

    /// Stops all new cross-chain signatures. Pending ones can still be denied.
    pub fn disable_eth(&mut self) {
        self.assert_owner();
        self.eth = None;
    }

    pub fn set_caps(&mut self, per_tx_cap: U128, daily_cap: U128) {
        self.assert_owner();
        self.per_tx_cap = NearToken::from_yoctonear(per_tx_cap.0);
        self.daily_cap = NearToken::from_yoctonear(daily_cap.0);
    }

    /// Arm the tripwire: `max_blocked` blocked requests within `window_secs`
    /// freeze the vault. With `cooldown_secs` the freeze lifts by itself after
    /// that long; without it, only the owner can unfreeze.
    pub fn set_tripwire(&mut self, max_blocked: u32, window_secs: u64, cooldown_secs: Option<u64>) {
        self.assert_owner();
        require!((1..=MAX_TRIPWIRE_COUNT).contains(&max_blocked), "max_blocked_out_of_range");
        require!((1..=MAX_TRIPWIRE_SECS).contains(&window_secs), "window_out_of_range");
        require!(
            cooldown_secs.map_or(true, |c| (1..=MAX_TRIPWIRE_SECS).contains(&c)),
            "cooldown_out_of_range"
        );
        self.tripwire = Some(Tripwire { max_blocked, window_secs, cooldown_secs });
        self.recent_blocked.clear();
    }

    pub fn disable_tripwire(&mut self) {
        self.assert_owner();
        self.tripwire = None;
        self.recent_blocked.clear();
    }

    /// Limit what one receiver can get per daily window, even a trusted one.
    /// Payments above it wait for the owner (`over_receiver_cap`).
    pub fn set_receiver_cap(&mut self, receiver: AccountId, daily_cap: U128) {
        self.assert_owner();
        self.receiver_caps
            .insert(receiver, NearToken::from_yoctonear(daily_cap.0));
    }

    pub fn remove_receiver_cap(&mut self, receiver: AccountId) {
        self.assert_owner();
        self.receiver_caps.remove(&receiver);
    }

    pub fn allow(&mut self, account: AccountId) {
        self.assert_owner();
        self.blocklist.remove(&account);
        self.allowlist.insert(account);
    }

    pub fn disallow(&mut self, account: AccountId) {
        self.assert_owner();
        self.allowlist.remove(&account);
    }

    pub fn block(&mut self, account: AccountId, reason: String) {
        self.assert_owner();
        self.allowlist.remove(&account);
        self.blocklist
            .insert(account, reason.chars().take(MAX_REASON_LEN).collect());
    }

    pub fn unblock(&mut self, account: AccountId) {
        self.assert_owner();
        self.blocklist.remove(&account);
    }

    /// Rotate the agent (for example, after its key's gas allowance runs out).
    pub fn set_agent(&mut self, agent: AccountId) {
        self.assert_owner();
        self.agent = agent;
    }

    pub fn set_owner(&mut self, owner: AccountId) {
        self.assert_owner();
        self.owner = owner;
    }

    /// Owner takes money back out. The agent has no path to this method.
    pub fn withdraw(&mut self, amount: U128) -> Promise {
        self.assert_owner();
        let amount = NearToken::from_yoctonear(amount.0);
        require!(amount <= self.available(), "insufficient_funds");
        Promise::new(self.owner.clone()).transfer(amount)
    }

    // ------------------------------------------------------------------
    // Callbacks
    // ------------------------------------------------------------------

    #[private]
    pub fn on_transfer(&mut self, id: u64) {
        if env::promise_result_checked(0, 0).is_err() {
            if let Some(mut r) = self.requests.get(id as u32).cloned() {
                let amount = NearToken::from_yoctonear(r.amount.0);
                self.spent_in_window = self.spent_in_window.saturating_sub(amount);
                let left = self.receiver_spent_now(&r.receiver).saturating_sub(amount);
                self.receiver_spent
                    .insert(r.receiver.clone(), (self.window_started_at, left));
                r.status = Status::Failed;
                r.rule = format!("{} -> transfer_failed", r.rule);
                self.stats.paid = self.stats.paid.saturating_sub(1);
                self.stats.near_paid = self.stats.near_paid.saturating_sub(r.amount.0);
                self.stats.failed += 1;
                self.requests.set(id as u32, r.clone());
                emit("spend_failed", &r);
            }
        }
    }

    /// The MPC signer answered (or timed out). Stores the signature so anyone
    /// can assemble and broadcast the transaction, or rolls the spend back.
    #[private]
    pub fn on_eth_signed(&mut self, id: u64) {
        let Some(mut r) = self.requests.get(id as u32).cloned() else {
            return;
        };
        let Some(mut tx) = self.eth_txs.get(&id).cloned() else {
            return;
        };
        let signature = env::promise_result_checked(0, MAX_SIGNATURE_LEN)
            .ok()
            .and_then(|bytes| parse_signature(&bytes));
        match signature {
            Some(sig) => {
                tx.signature = Some(sig);
                r.status = if r.rule.contains("approved_by_owner") {
                    Status::Approved
                } else {
                    Status::Executed
                };
                r.resolved_at = Some(env::block_timestamp());
                self.eth_txs.insert(id, tx);
                self.requests.set(id as u32, r.clone());
                emit("spend_signed", &r);
            }
            None => {
                let amount = r.amount.0;
                if self.window_live() {
                    self.eth_spent_in_window = self.eth_spent_in_window.saturating_sub(amount);
                }
                let left = self.receiver_spent_now(&r.receiver).as_yoctonear().saturating_sub(amount);
                self.receiver_spent.insert(
                    r.receiver.clone(),
                    (self.window_started_at, NearToken::from_yoctonear(left)),
                );
                // Give the nonce back if nothing was signed after it.
                if let (Some(p), Some(n)) = (self.eth.as_mut(), tx.nonce) {
                    if p.next_nonce == n + 1 {
                        p.next_nonce = n;
                    }
                }
                r.status = Status::Failed;
                r.rule = format!("{} -> signature_failed", r.rule);
                r.resolved_at = Some(env::block_timestamp());
                self.stats.paid = self.stats.paid.saturating_sub(1);
                self.wei_paid = self.wei_paid.saturating_sub(amount);
                self.stats.failed += 1;
                self.requests.set(id as u32, r.clone());
                emit("spend_failed", &r);
            }
        }
    }

    // ------------------------------------------------------------------
    // Views
    // ------------------------------------------------------------------

    /// Dry run: what `request_spend` would decide for this receiver and amount
    /// right now. Free to call, changes nothing, same code path.
    pub fn check_spend(&self, receiver: AccountId, amount: U128, chain: Option<String>) -> Decision {
        require!(amount.0 > 0, "amount_must_be_positive");
        let eth = parse_chain(chain.as_deref(), &receiver);
        self.decide(&receiver, amount.0, eth)
    }

    pub fn get_vault(&self) -> VaultView {
        VaultView {
            owner: self.owner.clone(),
            agent: self.agent.clone(),
            per_tx_cap: U128(self.per_tx_cap.as_yoctonear()),
            daily_cap: U128(self.daily_cap.as_yoctonear()),
            spent_today: U128(self.window_spent().as_yoctonear()),
            window_started_at: self.window_started_at,
            frozen: self.is_frozen(),
            balance: U128(env::account_balance().as_yoctonear()),
            available: U128(self.available().as_yoctonear()),
            allowlist: self.allowlist.iter().cloned().collect(),
            blocklist: self
                .blocklist
                .iter()
                .map(|(a, r)| (a.clone(), r.clone()))
                .collect(),
            open_pending: self.open_pending,
            total_requests: self.requests.len() as u64,
            receiver_caps: self
                .receiver_caps
                .iter()
                .map(|(a, cap)| ReceiverCapView {
                    receiver: a.clone(),
                    daily_cap: U128(cap.as_yoctonear()),
                    spent_today: U128(self.receiver_spent_now(a).as_yoctonear()),
                })
                .collect(),
            frozen_by_owner: self.frozen,
            tripped_until: self.tripped_until.filter(|t| *t > env::block_timestamp()),
            tripwire: self.tripwire.clone(),
            recent_blocked: self.live_blocks(env::block_timestamp()).len() as u32,
            stats: StatsView {
                requests: self.stats.requests,
                paid: self.stats.paid,
                near_paid: U128(self.stats.near_paid),
                held: self.stats.held,
                denied: self.stats.denied,
                near_denied: U128(self.stats.near_denied),
                blocked: self.stats.blocked,
                near_blocked: U128(self.stats.near_blocked),
                failed: self.stats.failed,
                tripwire_trips: self.stats.tripwire_trips,
            },
            eth: self.eth.clone(),
            eth_spent_today: U128(self.eth_window_spent()),
            wei_paid: U128(self.wei_paid),
            wei_refused: U128(self.wei_refused),
        }
    }

    /// Newest first. `before` is an exclusive request id for paging.
    pub fn get_requests(&self, before: Option<u64>, limit: Option<u64>) -> Vec<RequestView> {
        let len = self.requests.len() as u64;
        let end = before.unwrap_or(len).min(len);
        let limit = limit.unwrap_or(20).min(100);
        (end.saturating_sub(limit)..end)
            .rev()
            .filter_map(|i| self.get_request(i))
            .collect()
    }

    pub fn get_request(&self, id: u64) -> Option<RequestView> {
        let request = self.requests.get(id as u32).cloned()?;
        let eth = self.eth_txs.get(&id).cloned();
        Some(RequestView {
            request,
            chain: if eth.is_some() { "ethereum" } else { "near" }.to_string(),
            eth,
        })
    }

    // ------------------------------------------------------------------
    // Internal
    // ------------------------------------------------------------------

    fn assert_owner(&self) {
        require!(env::predecessor_account_id() == self.owner, "only_owner");
    }

    /// The policy, in order. Shared by `request_spend` and `check_spend`.
    /// `amount` is yoctoNEAR, or wei when `eth` is true. A receiver's own
    /// limit is in the unit of its chain.
    fn decide(&self, receiver: &AccountId, amount: u128, eth: bool) -> Decision {
        let amount_t = NearToken::from_yoctonear(amount);
        let over_receiver_cap = self
            .receiver_caps
            .get(receiver)
            .is_some_and(|cap| self.receiver_spent_now(receiver).saturating_add(amount_t) > *cap);
        // (per-payment cap, daily cap, spent today) in the chain's own unit.
        let caps = if eth {
            self.eth
                .as_ref()
                .map(|p| (p.per_tx_cap.0, p.daily_cap.0, self.eth_window_spent()))
        } else {
            Some((
                self.per_tx_cap.as_yoctonear(),
                self.daily_cap.as_yoctonear(),
                self.window_spent().as_yoctonear(),
            ))
        };
        let (status, rule) = if self.frozen {
            (Status::Blocked, "vault_frozen".to_string())
        } else if self.is_frozen() {
            (Status::Blocked, "tripwire_frozen".to_string())
        } else if let Some(why) = self.blocklist.get(receiver) {
            (Status::Blocked, format!("receiver_blocklisted: {why}"))
        } else if caps.is_none() {
            (Status::Blocked, "chain_not_enabled".to_string())
        } else if !eth && amount_t > self.available() {
            (Status::Blocked, "insufficient_funds".to_string())
        } else if caps.is_some_and(|(per_tx, _, _)| amount > per_tx) {
            self.pending_or_blocked("over_per_tx_cap")
        } else if caps.is_some_and(|(_, daily, spent)| spent.saturating_add(amount) > daily) {
            self.pending_or_blocked("over_daily_cap")
        } else if over_receiver_cap {
            self.pending_or_blocked("over_receiver_cap")
        } else if !self.allowlist.contains(receiver) {
            self.pending_or_blocked("new_receiver")
        } else {
            (Status::Executed, "within_policy".to_string())
        };
        Decision { status, rule }
    }

    /// Frozen by the owner, or held by the tripwire right now.
    fn is_frozen(&self) -> bool {
        self.frozen || self.tripped_until.is_some_and(|t| env::block_timestamp() < t)
    }

    /// Blocked-request timestamps still inside the tripwire window.
    fn live_blocks(&self, now: u64) -> Vec<u64> {
        match &self.tripwire {
            Some(t) => {
                let since = now.saturating_sub(t.window_secs.saturating_mul(1_000_000_000));
                self.recent_blocked.iter().copied().filter(|ts| *ts > since).collect()
            }
            None => Vec::new(),
        }
    }

    /// Counts a blocked request. Returns true if this one tripped the wire.
    fn note_block_and_maybe_trip(&mut self) -> bool {
        let Some(t) = self.tripwire.clone() else {
            return false;
        };
        let now = env::block_timestamp();
        let mut live = self.live_blocks(now);
        live.push(now);
        if live.len() as u32 >= t.max_blocked {
            self.recent_blocked.clear();
            let until = t
                .cooldown_secs
                .map_or(u64::MAX, |c| now.saturating_add(c.saturating_mul(1_000_000_000)));
            self.tripped_until = Some(until);
            self.stats.tripwire_trips += 1;
            let data = near_sdk::serde_json::json!({
                "standard": "leash",
                "version": "1.2.0",
                "event": "tripwire_tripped",
                "data": [{ "blocked": live.len(), "window_secs": t.window_secs, "until": until.to_string() }],
            });
            log!("EVENT_JSON:{}", data);
            true
        } else {
            let keep = live.len().saturating_sub(t.max_blocked as usize);
            self.recent_blocked = live.split_off(keep);
            false
        }
    }

    fn window_live(&self) -> bool {
        env::block_timestamp() < self.window_started_at + DAY_NS
    }

    /// Spent in the current window; zero once the window has expired.
    fn window_spent(&self) -> NearToken {
        if self.window_live() {
            self.spent_in_window
        } else {
            NearToken::from_yoctonear(0)
        }
    }

    fn receiver_spent_now(&self, receiver: &AccountId) -> NearToken {
        match self.receiver_spent.get(receiver) {
            Some((started, spent)) if *started == self.window_started_at && self.window_live() => {
                *spent
            }
            _ => NearToken::from_yoctonear(0),
        }
    }

    fn record_spend(&mut self, receiver: &AccountId, amount: NearToken) {
        self.spent_in_window = self.spent_in_window.saturating_add(amount);
        let total = self.receiver_spent_now(receiver).saturating_add(amount);
        self.receiver_spent
            .insert(receiver.clone(), (self.window_started_at, total));
    }

    fn roll_window(&mut self) {
        if env::block_timestamp() >= self.window_started_at + DAY_NS {
            self.window_started_at = env::block_timestamp();
            self.spent_in_window = NearToken::from_yoctonear(0);
            self.eth_spent_in_window = 0;
        }
    }

    fn eth_window_spent(&self) -> u128 {
        if self.window_live() {
            self.eth_spent_in_window
        } else {
            0
        }
    }

    fn record_eth_spend(&mut self, receiver: &AccountId, wei: u128) {
        self.eth_spent_in_window = self.eth_spent_in_window.saturating_add(wei);
        let total = self
            .receiver_spent_now(receiver)
            .saturating_add(NearToken::from_yoctonear(wei));
        self.receiver_spent
            .insert(receiver.clone(), (self.window_started_at, total));
    }

    /// Fixes the nonce and fees, builds and hashes the EIP-1559 transaction,
    /// and asks the MPC signer to sign that hash under `ETH_PATH`.
    fn start_signing(&mut self, id: u64) {
        let p = self.eth.as_mut().unwrap_or_else(|| env::panic_str("chain_not_enabled"));
        let mut tx = self
            .eth_txs
            .get(&id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("no_such_eth_tx"));
        tx.chain_id = p.chain_id;
        tx.nonce = Some(p.next_nonce);
        tx.max_fee_per_gas = p.max_fee_per_gas;
        tx.max_priority_fee_per_gas = p.max_priority_fee_per_gas;
        p.next_nonce += 1;
        let mpc = p.mpc_contract.clone();
        let payload = to_hex(&eip1559_signing_hash(&tx));
        tx.payload = Some(payload.clone());
        self.eth_txs.insert(id, tx);
        let args = near_sdk::serde_json::json!({
            "request": { "path": ETH_PATH, "payload_v2": { "Ecdsa": payload }, "domain_id": 0 }
        });
        let _ = Promise::new(mpc)
            .function_call(
                "sign".to_string(),
                args.to_string().into_bytes(),
                NearToken::from_yoctonear(1),
                MPC_SIGN_GAS,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(ON_SIGNED_GAS)
                    .on_eth_signed(id),
            );
    }

    fn available(&self) -> NearToken {
        let storage = env::storage_byte_cost().saturating_mul(env::storage_usage() as u128);
        env::account_balance()
            .saturating_sub(storage)
            .saturating_sub(RESERVE)
    }

    fn pending_or_blocked(&self, rule: &str) -> (Status, String) {
        if self.open_pending >= MAX_OPEN_PENDING {
            (Status::Blocked, format!("{rule}; too_many_open_requests"))
        } else {
            (Status::Pending, rule.to_string())
        }
    }

    fn take_pending(&mut self, id: u64) -> Request {
        let r = self
            .requests
            .get(id as u32)
            .cloned()
            .unwrap_or_else(|| env::panic_str("no_such_request"));
        require!(r.status == Status::Pending, "not_pending");
        self.open_pending = self.open_pending.saturating_sub(1);
        r
    }

    fn pay(&self, id: u64, receiver: &AccountId, amount: NearToken) {
        let _ = Promise::new(receiver.clone()).transfer(amount).then(
            Self::ext(env::current_account_id())
                .with_static_gas(CALLBACK_GAS)
                .on_transfer(id),
        );
    }
}

fn emit(event: &str, r: &Request) {
    let data = near_sdk::serde_json::json!({
        "standard": "leash",
        "version": "1.0.0",
        "event": event,
        "data": [r],
    });
    log!("EVENT_JSON:{}", data);
}

/// True for an Ethereum request. Panics on an unknown chain or a receiver
/// that is not a lowercase 0x address (malformed input, not a policy call).
fn parse_chain(chain: Option<&str>, receiver: &AccountId) -> bool {
    match chain {
        None | Some("near") => false,
        Some("ethereum") => {
            require!(parse_eth_address(receiver.as_str()).is_some(), "invalid_eth_address");
            true
        }
        Some(_) => env::panic_str("unsupported_chain"),
    }
}

fn parse_eth_address(s: &str) -> Option<[u8; 20]> {
    let hex = s.strip_prefix("0x")?;
    if hex.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads `{ big_r: { affine_point }, s: { scalar }, recovery_id }` from the MPC signer.
fn parse_signature(bytes: &[u8]) -> Option<EthSignature> {
    let v: near_sdk::serde_json::Value = near_sdk::serde_json::from_slice(bytes).ok()?;
    Some(EthSignature {
        big_r: v.get("big_r")?.get("affine_point")?.as_str()?.to_string(),
        s: v.get("s")?.get("scalar")?.as_str()?.to_string(),
        recovery_id: u8::try_from(v.get("recovery_id")?.as_u64()?).ok()?,
    })
}

/// keccak256(0x02 || rlp([chain_id, nonce, max_priority_fee, max_fee, gas,
/// to, value, data, access_list])): the hash an EIP-1559 signature covers.
fn eip1559_signing_hash(tx: &EthTx) -> Vec<u8> {
    let to = parse_eth_address(&tx.to).unwrap_or_else(|| env::panic_str("invalid_eth_address"));
    let mut body = Vec::new();
    rlp_uint(&mut body, tx.chain_id as u128);
    rlp_uint(&mut body, tx.nonce.unwrap_or_else(|| env::panic_str("no_nonce")) as u128);
    rlp_uint(&mut body, tx.max_priority_fee_per_gas.0);
    rlp_uint(&mut body, tx.max_fee_per_gas.0);
    rlp_uint(&mut body, tx.gas_limit as u128);
    rlp_bytes(&mut body, &to);
    rlp_uint(&mut body, tx.value.0);
    rlp_bytes(&mut body, &[]); // data
    body.push(0xc0); // empty access list
    let mut out = vec![0x02];
    rlp_len(&mut out, body.len(), 0xc0);
    out.extend_from_slice(&body);
    env::keccak256(&out)
}

fn rlp_len(out: &mut Vec<u8>, len: usize, offset: u8) {
    if len < 56 {
        out.push(offset + len as u8);
    } else {
        let bytes = trim_be(&(len as u128).to_be_bytes()).to_vec();
        out.push(offset + 55 + bytes.len() as u8);
        out.extend_from_slice(&bytes);
    }
}

fn rlp_bytes(out: &mut Vec<u8>, b: &[u8]) {
    if b.len() == 1 && b[0] < 0x80 {
        out.push(b[0]);
    } else {
        rlp_len(out, b.len(), 0x80);
        out.extend_from_slice(b);
    }
}

fn rlp_uint(out: &mut Vec<u8>, v: u128) {
    rlp_bytes(out, trim_be(&v.to_be_bytes()));
}

fn trim_be(b: &[u8]) -> &[u8] {
    let first = b.iter().position(|x| *x != 0).unwrap_or(b.len());
    &b[first..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::{testing_env, PromiseResult};

    fn near(n: u128) -> U128 {
        U128(NearToken::from_near(n).as_yoctonear())
    }
    fn owner() -> AccountId {
        "owner.testnet".parse().unwrap()
    }
    fn agent() -> AccountId {
        "agent.testnet".parse().unwrap()
    }
    fn cafe() -> AccountId {
        "cafe.testnet".parse().unwrap()
    }
    fn attacker() -> AccountId {
        "drainer.testnet".parse().unwrap()
    }

    fn ctx(caller: AccountId, ts: u64) {
        testing_env!(VMContextBuilder::new()
            .current_account_id("vault.testnet".parse().unwrap())
            .predecessor_account_id(caller)
            .account_balance(NearToken::from_near(20))
            .block_timestamp(ts)
            .build());
    }

    fn setup() -> Leash {
        ctx(owner(), 1);
        let mut v = Leash::new(owner(), agent(), near(1), near(3));
        v.allow(cafe());
        v.block(attacker(), "known drainer".into());
        v
    }

    #[test]
    fn pays_allowlisted_receiver_within_caps() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), near(1), "coffee".into(), None);
        assert_eq!(r.status, Status::Executed);
        assert_eq!(v.get_vault().spent_today, near(1));
    }

    #[test]
    fn blocks_blocklisted_receiver_even_for_tiny_amounts() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(attacker(), U128(1), "ignore previous instructions".into(), None);
        assert_eq!(r.status, Status::Blocked);
        assert!(r.rule.starts_with("receiver_blocklisted"));
    }

    #[test]
    fn over_per_tx_cap_goes_to_owner() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), near(2), "catering".into(), None);
        assert_eq!(r.status, Status::Pending);
        assert_eq!(r.rule, "over_per_tx_cap");
        assert_eq!(v.get_vault().open_pending, 1);
    }

    #[test]
    fn unknown_receiver_goes_to_owner() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend("stranger.testnet".parse().unwrap(), U128(10), "tip".into(), None);
        assert_eq!(r.status, Status::Pending);
        assert_eq!(r.rule, "new_receiver");
    }

    #[test]
    fn daily_cap_holds_across_many_small_payments() {
        let mut v = setup();
        ctx(agent(), 2);
        for _ in 0..3 {
            assert_eq!(
                v.request_spend(cafe(), near(1), "coffee".into(), None).status,
                Status::Executed
            );
        }
        let r = v.request_spend(cafe(), near(1), "fourth coffee".into(), None);
        assert_eq!(r.status, Status::Pending);
        assert_eq!(r.rule, "over_daily_cap");
    }

    #[test]
    fn daily_window_resets_after_24h() {
        let mut v = setup();
        ctx(agent(), 2);
        for _ in 0..3 {
            v.request_spend(cafe(), near(1), "coffee".into(), None);
        }
        ctx(agent(), 2 + DAY_NS);
        assert_eq!(
            v.request_spend(cafe(), near(1), "next day".into(), None).status,
            Status::Executed
        );
    }

    #[test]
    fn freeze_blocks_everything() {
        let mut v = setup();
        v.freeze();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), U128(1), "coffee".into(), None);
        assert_eq!(r.status, Status::Blocked);
        assert_eq!(r.rule, "vault_frozen");
    }

    #[test]
    fn cannot_spend_more_than_free_balance() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), near(50), "everything".into(), None);
        assert_eq!(r.status, Status::Blocked);
        assert_eq!(r.rule, "insufficient_funds");
    }

    #[test]
    fn owner_approves_and_denies() {
        let mut v = setup();
        ctx(agent(), 2);
        let a = v.request_spend(cafe(), near(2), "catering".into(), None);
        let b = v.request_spend("stranger.testnet".parse().unwrap(), U128(5), "tip".into(), None);
        ctx(owner(), 3);
        assert_eq!(v.approve(a.id).status, Status::Approved);
        assert_eq!(v.deny(b.id).status, Status::Denied);
        assert_eq!(v.get_vault().open_pending, 0);
    }

    #[test]
    #[should_panic(expected = "only_owner")]
    fn agent_cannot_approve_its_own_request() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), near(2), "catering".into(), None);
        v.approve(r.id);
    }

    #[test]
    #[should_panic(expected = "only_owner")]
    fn agent_cannot_edit_the_allowlist() {
        let mut v = setup();
        ctx(agent(), 2);
        v.allow(attacker());
    }

    #[test]
    fn owner_reset_starts_a_fresh_daily_window() {
        let mut v = setup();
        ctx(agent(), 2);
        for _ in 0..3 {
            v.request_spend(cafe(), near(1), "coffee".into(), None);
        }
        assert_eq!(v.request_spend(cafe(), near(1), "fourth".into(), None).rule, "over_daily_cap");
        ctx(owner(), 3);
        v.reset_window();
        assert_eq!(v.get_vault().spent_today, U128(0));
        ctx(agent(), 4);
        assert_eq!(v.request_spend(cafe(), near(1), "after reset".into(), None).status, Status::Executed);
    }

    #[test]
    #[should_panic(expected = "only_owner")]
    fn agent_cannot_reset_its_daily_window() {
        let mut v = setup();
        ctx(agent(), 2);
        v.reset_window();
    }

    #[test]
    #[should_panic(expected = "only_owner")]
    fn agent_cannot_withdraw() {
        let mut v = setup();
        ctx(agent(), 2);
        let _ = v.withdraw(near(1));
    }

    #[test]
    #[should_panic(expected = "only_agent")]
    fn strangers_cannot_request() {
        let mut v = setup();
        ctx(attacker(), 2);
        v.request_spend(attacker(), near(1), "hi".into(), None);
    }

    #[test]
    #[should_panic(expected = "not_pending")]
    fn cannot_approve_twice() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), near(2), "catering".into(), None);
        ctx(owner(), 3);
        v.approve(r.id);
        v.approve(r.id);
    }

    #[test]
    fn open_pending_is_capped() {
        let mut v = setup();
        ctx(agent(), 2);
        for i in 0..MAX_OPEN_PENDING {
            assert_eq!(
                v.request_spend(cafe(), near(2), format!("spam {i}"), None).status,
                Status::Pending
            );
        }
        let r = v.request_spend(cafe(), near(2), "one more".into(), None);
        assert_eq!(r.status, Status::Blocked);
    }

    fn books() -> AccountId {
        "books.testnet".parse().unwrap()
    }

    #[test]
    fn receiver_cap_limits_a_trusted_receiver() {
        let mut v = setup();
        v.allow(books());
        v.set_receiver_cap(books(), near(1));
        ctx(agent(), 2);
        assert_eq!(v.request_spend(books(), near(1), "book".into(), None).status, Status::Executed);
        let r = v.request_spend(books(), U128(1), "one more".into(), None);
        assert_eq!(r.status, Status::Pending);
        assert_eq!(r.rule, "over_receiver_cap");
        // Other receivers keep the rest of the vault-wide allowance.
        assert_eq!(v.request_spend(cafe(), near(1), "coffee".into(), None).status, Status::Executed);
        let caps = v.get_vault().receiver_caps;
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].spent_today, near(1));
    }

    #[test]
    fn receiver_cap_clears_with_the_daily_window() {
        let mut v = setup();
        v.allow(books());
        v.set_receiver_cap(books(), near(1));
        ctx(agent(), 2);
        v.request_spend(books(), near(1), "book".into(), None);
        ctx(agent(), 2 + DAY_NS);
        assert_eq!(v.get_vault().receiver_caps[0].spent_today, U128(0));
        assert_eq!(v.request_spend(books(), near(1), "next day".into(), None).status, Status::Executed);
        ctx(owner(), 3 + DAY_NS);
        v.reset_window();
        assert_eq!(v.get_vault().receiver_caps[0].spent_today, U128(0));
    }

    #[test]
    #[should_panic(expected = "only_owner")]
    fn agent_cannot_raise_a_receiver_cap() {
        let mut v = setup();
        ctx(agent(), 2);
        v.set_receiver_cap(cafe(), near(100));
    }

    #[test]
    fn check_spend_matches_request_spend_and_writes_nothing() {
        let mut v = setup();
        v.allow(books());
        v.set_receiver_cap(books(), U128(5));
        ctx(agent(), 2);
        let cases = [
            (cafe(), near(1)),
            (cafe(), near(2)),
            (attacker(), U128(1)),
            ("stranger.testnet".parse().unwrap(), U128(1)),
            (books(), U128(9)),
            (cafe(), near(50)),
        ];
        for (who, amount) in cases {
            let before = v.get_vault().total_requests;
            let d = v.check_spend(who.clone(), amount, None);
            assert_eq!(v.get_vault().total_requests, before, "check_spend must not write");
            let r = v.request_spend(who, amount, "x".into(), None);
            assert_eq!((d.status, d.rule), (r.status, r.rule));
        }
    }

    #[test]
    fn check_spend_sees_an_expired_window_as_fresh() {
        let mut v = setup();
        ctx(agent(), 2);
        for _ in 0..3 {
            v.request_spend(cafe(), near(1), "coffee".into(), None);
        }
        assert_eq!(v.check_spend(cafe(), near(1), None).rule, "over_daily_cap");
        ctx(agent(), 2 + DAY_NS);
        assert_eq!(v.check_spend(cafe(), near(1), None).status, Status::Executed);
    }

    #[test]
    fn migrate_keeps_v1_2_state_and_starts_without_cross_chain() {
        ctx(owner(), 1);
        let mut allowlist = IterableSet::new(Prefix::Allowlist);
        allowlist.insert(cafe());
        allowlist.flush();
        let mut requests: Vector<Request> = Vector::new(Prefix::Requests);
        let old_request = |id: u64, status: Status, amount: U128| Request {
            id,
            receiver: cafe(),
            amount,
            reason: "old".into(),
            status,
            rule: "within_policy".into(),
            created_at: 1,
            resolved_at: Some(1),
        };
        requests.push(old_request(0, Status::Executed, near(1)));
        requests.push(old_request(1, Status::Blocked, near(5)));
        requests.push(old_request(2, Status::Denied, near(2)));
        requests.push(old_request(3, Status::Approved, near(2)));
        requests.flush();
        let mut receiver_caps = IterableMap::new(Prefix::ReceiverCaps);
        receiver_caps.insert(cafe(), NearToken::from_near(2));
        receiver_caps.flush();
        let tripwire = Tripwire { max_blocked: 3, window_secs: 600, cooldown_secs: Some(120) };
        let stats = Stats {
            requests: 4,
            paid: 2,
            near_paid: near(3).0,
            held: 2,
            denied: 1,
            near_denied: near(2).0,
            blocked: 1,
            near_blocked: near(5).0,
            failed: 0,
            tripwire_trips: 2,
        };
        let old = LeashV12 {
            owner: owner(),
            agent: agent(),
            per_tx_cap: NearToken::from_near(1),
            daily_cap: NearToken::from_near(3),
            spent_in_window: NearToken::from_near(1),
            window_started_at: 1,
            frozen: false,
            open_pending: 0,
            allowlist,
            blocklist: IterableMap::new(Prefix::Blocklist),
            requests,
            receiver_caps,
            receiver_spent: LookupMap::new(Prefix::ReceiverSpent),
            tripwire: Some(tripwire.clone()),
            tripped_until: None,
            recent_blocked: vec![1],
            stats,
        };
        env::state_write(&old);
        ctx("vault.testnet".parse().unwrap(), 2);
        let mut v = Leash::migrate();
        let view = v.get_vault();
        assert_eq!(view.owner, owner());
        assert_eq!(view.spent_today, near(1));
        assert_eq!(view.allowlist, vec![cafe()]);
        assert_eq!(view.total_requests, 4);
        assert_eq!(view.receiver_caps.len(), 1);
        assert_eq!(view.tripwire, Some(tripwire));
        assert_eq!(view.eth, None);
        let s = view.stats;
        assert_eq!((s.requests, s.paid, s.blocked, s.denied, s.held, s.tripwire_trips), (4, 2, 1, 1, 2, 2));
        assert_eq!(s.near_paid, near(3));
        let first = v.get_request(0).unwrap();
        assert_eq!((first.request.reason.as_str(), first.chain.as_str()), ("old", "near"));
        ctx(agent(), 3);
        assert_eq!(v.request_spend(cafe(), near(1), "after".into(), None).status, Status::Executed);
        let r = v.request_spend(eth_shop(), U128(1), "x".into(), eth());
        assert_eq!((r.status, r.rule.as_str()), (Status::Blocked, "chain_not_enabled"));
    }

    // ------------------------------------------------------------------
    // Cross-chain (Chain Signatures)
    // ------------------------------------------------------------------

    const REFERENCE_HASH: &str = "f6fa7cd2975ca30c24079442e70c3a4ebd7b7f2367dc6fb0202d7e16def15223";
    const REFERENCE_HASH_ZERO: &str = "a16383b3cf3a4578f9ec6bebf9966623ac621d3bc33d504060cc07380d5ec753";
    const GWEI: u128 = 1_000_000_000;
    const MILLI_ETH: u128 = 1_000_000_000_000_000;

    fn eth() -> Option<String> {
        Some("ethereum".into())
    }
    fn eth_shop() -> AccountId {
        "0x1111111111111111111111111111111111111111".parse().unwrap()
    }
    fn eth_stranger() -> AccountId {
        "0x2222222222222222222222222222222222222222".parse().unwrap()
    }
    fn eth_drainer() -> AccountId {
        "0x3333333333333333333333333333333333333333".parse().unwrap()
    }

    /// Vault with Sepolia enabled: 5 mETH per payment, 10 mETH a day.
    fn with_eth() -> Leash {
        let mut v = setup();
        v.enable_eth(
            11155111,
            "v1.signer-prod.testnet".parse().unwrap(),
            U128(5 * MILLI_ETH),
            U128(10 * MILLI_ETH),
            U128(30 * GWEI),
            U128(2 * GWEI),
            7,
        );
        v.allow(eth_shop());
        v.block(eth_drainer(), "known drainer".into());
        v
    }

    fn callback(v: &mut Leash, id: u64, result: PromiseResult) {
        testing_env!(
            VMContextBuilder::new()
                .current_account_id("vault.testnet".parse().unwrap())
                .predecessor_account_id("vault.testnet".parse().unwrap())
                .account_balance(NearToken::from_near(20))
                .block_timestamp(10)
                .build(),
            near_sdk::test_vm_config(),
            near_sdk::RuntimeFeesConfig::test(),
            Default::default(),
            vec![result],
        );
        v.on_eth_signed(id);
    }

    const MPC_ANSWER: &str = r#"{"scheme":"Secp256k1","big_r":{"affine_point":"0382fcb976b61247987d41661932a2a20c5b01486b35fc92132f09312e45bea9e5"},"s":{"scalar":"136c86e20eff6de1766cca76107b3501edb10fa49614612ba6d6cec481b72726"},"recovery_id":0}"#;

    #[test]
    fn eip1559_hash_matches_a_reference_encoder() {
        ctx(owner(), 1);
        // Reference: viem keccak256(serializeTransaction({...})) for the same fields.
        let tx = EthTx {
            chain_id: 11155111,
            to: "0x1111111111111111111111111111111111111111".into(),
            value: U128(MILLI_ETH),
            gas_limit: 21_000,
            max_fee_per_gas: U128(30 * GWEI),
            max_priority_fee_per_gas: U128(2 * GWEI),
            nonce: Some(7),
            payload: None,
            signature: None,
        };
        assert_eq!(to_hex(&eip1559_signing_hash(&tx)), REFERENCE_HASH);
        let zero = EthTx { nonce: Some(0), value: U128(0), ..tx };
        assert_eq!(to_hex(&eip1559_signing_hash(&zero)), REFERENCE_HASH_ZERO);
    }

    #[test]
    fn eth_payment_inside_policy_goes_to_the_mpc_signer() {
        let mut v = with_eth();
        ctx(agent(), 2);
        let r = v.request_spend(eth_shop(), U128(MILLI_ETH), "api credits".into(), eth());
        assert_eq!((r.status, r.rule.as_str()), (Status::Signing, "within_policy"));
        let view = v.get_request(r.id).unwrap();
        assert_eq!(view.chain, "ethereum");
        let tx = view.eth.unwrap();
        assert_eq!((tx.nonce, tx.gas_limit, tx.value), (Some(7), 21_000, U128(MILLI_ETH)));
        assert_eq!(tx.payload.as_deref(), Some(REFERENCE_HASH));
        let vault = v.get_vault();
        assert_eq!(vault.eth.unwrap().next_nonce, 8);
        assert_eq!(vault.eth_spent_today, U128(MILLI_ETH));
        // The NEAR allowance is untouched.
        assert_eq!(vault.spent_today, U128(0));
    }

    #[test]
    fn mpc_signature_is_stored_for_anyone_to_broadcast() {
        let mut v = with_eth();
        ctx(agent(), 2);
        let id = v.request_spend(eth_shop(), U128(MILLI_ETH), "x".into(), eth()).id;
        callback(&mut v, id, PromiseResult::Successful(MPC_ANSWER.as_bytes().to_vec()));
        let view = v.get_request(id).unwrap();
        assert_eq!(view.request.status, Status::Executed);
        let sig = view.eth.unwrap().signature.unwrap();
        assert_eq!(sig.recovery_id, 0);
        assert!(sig.big_r.starts_with("0382fcb9"));
        assert_eq!(v.get_vault().stats.paid, 1);
    }

    #[test]
    fn failed_signature_rolls_back_spend_and_nonce() {
        let mut v = with_eth();
        ctx(agent(), 2);
        let id = v.request_spend(eth_shop(), U128(MILLI_ETH), "x".into(), eth()).id;
        callback(&mut v, id, PromiseResult::Failed);
        let view = v.get_request(id).unwrap();
        assert_eq!(view.request.status, Status::Failed);
        assert!(view.request.rule.ends_with("signature_failed"));
        let vault = v.get_vault();
        assert_eq!(vault.eth_spent_today, U128(0));
        assert_eq!(vault.eth.unwrap().next_nonce, 7);
        assert_eq!((vault.stats.paid, vault.stats.failed), (0, 1));
        assert_eq!(vault.wei_paid, U128(0));
    }

    #[test]
    fn garbage_from_the_signer_counts_as_a_failure() {
        let mut v = with_eth();
        ctx(agent(), 2);
        let id = v.request_spend(eth_shop(), U128(MILLI_ETH), "x".into(), eth()).id;
        callback(&mut v, id, PromiseResult::Successful(b"{\"nope\":1}".to_vec()));
        assert_eq!(v.get_request(id).unwrap().request.status, Status::Failed);
    }

    #[test]
    fn eth_caps_are_separate_from_near_caps() {
        let mut v = with_eth();
        ctx(agent(), 2);
        // Spend the whole NEAR daily cap: Ethereum is unaffected.
        for _ in 0..3 {
            v.request_spend(cafe(), near(1), "coffee".into(), None);
        }
        let r = v.request_spend(eth_shop(), U128(5 * MILLI_ETH), "x".into(), eth());
        assert_eq!(r.status, Status::Signing);
        let r = v.request_spend(eth_shop(), U128(6 * MILLI_ETH), "x".into(), eth());
        assert_eq!((r.status, r.rule.as_str()), (Status::Pending, "over_per_tx_cap"));
        let r = v.request_spend(eth_shop(), U128(5 * MILLI_ETH), "x".into(), eth());
        assert_eq!(r.status, Status::Signing);
        let r = v.request_spend(eth_shop(), U128(1), "x".into(), eth());
        assert_eq!((r.status, r.rule.as_str()), (Status::Pending, "over_daily_cap"));
    }

    #[test]
    fn unknown_eth_receiver_waits_and_approval_signs() {
        let mut v = with_eth();
        ctx(agent(), 2);
        let r = v.request_spend(eth_stranger(), U128(MILLI_ETH), "refund".into(), eth());
        assert_eq!((r.status, r.rule.as_str()), (Status::Pending, "new_receiver"));
        assert_eq!(v.get_request(r.id).unwrap().eth.unwrap().nonce, None);
        ctx(owner(), 3);
        let a = v.approve(r.id);
        assert_eq!(a.status, Status::Signing);
        assert_eq!(v.get_request(r.id).unwrap().eth.unwrap().nonce, Some(7));
        callback(&mut v, r.id, PromiseResult::Successful(MPC_ANSWER.as_bytes().to_vec()));
        assert_eq!(v.get_request(r.id).unwrap().request.status, Status::Approved);
    }

    #[test]
    fn denied_eth_request_is_never_signed() {
        let mut v = with_eth();
        ctx(agent(), 2);
        let id = v.request_spend(eth_stranger(), U128(MILLI_ETH), "x".into(), eth()).id;
        ctx(owner(), 3);
        v.deny(id);
        let view = v.get_request(id).unwrap();
        assert_eq!(view.request.status, Status::Denied);
        assert_eq!(view.eth.unwrap().payload, None);
        assert_eq!(v.get_vault().wei_refused, U128(MILLI_ETH));
    }

    #[test]
    fn blocklisted_eth_address_is_blocked_and_feeds_the_tripwire() {
        let mut v = with_eth();
        v.set_tripwire(2, 600, None);
        ctx(agent(), 2);
        let r = v.request_spend(eth_drainer(), U128(1), "x".into(), eth());
        assert_eq!(r.status, Status::Blocked);
        let view = v.get_request(r.id).unwrap();
        assert_eq!(view.chain, "ethereum");
        assert_eq!(view.eth.unwrap().payload, None);
        let r = v.request_spend(eth_drainer(), U128(1), "x".into(), eth());
        assert!(r.rule.ends_with("tripwire_tripped"));
        // Frozen covers both chains.
        let r = v.request_spend(cafe(), near(1), "coffee".into(), None);
        assert_eq!((r.status, r.rule.as_str()), (Status::Blocked, "tripwire_frozen"));
        let r = v.request_spend(eth_shop(), U128(1), "x".into(), eth());
        assert_eq!((r.status, r.rule.as_str()), (Status::Blocked, "tripwire_frozen"));
    }

    #[test]
    fn check_spend_matches_for_ethereum() {
        let mut v = with_eth();
        ctx(agent(), 2);
        for (to, amount) in [(eth_shop(), MILLI_ETH), (eth_shop(), 6 * MILLI_ETH), (eth_stranger(), 1), (eth_drainer(), 1)] {
            let d = v.check_spend(to.clone(), U128(amount), eth());
            let r = v.request_spend(to, U128(amount), "x".into(), eth());
            let expected = if r.status == Status::Signing { Status::Executed } else { r.status };
            assert_eq!((d.status, d.rule), (expected, r.rule));
        }
    }

    #[test]
    #[should_panic(expected = "invalid_eth_address")]
    fn ethereum_needs_a_0x_address() {
        let mut v = with_eth();
        ctx(agent(), 2);
        v.request_spend(cafe(), U128(1), "x".into(), eth());
    }

    #[test]
    #[should_panic(expected = "unsupported_chain")]
    fn unknown_chain_is_rejected() {
        let mut v = with_eth();
        ctx(agent(), 2);
        v.request_spend(eth_shop(), U128(1), "x".into(), Some("solana".into()));
    }

    #[test]
    #[should_panic(expected = "only_owner")]
    fn agent_cannot_enable_ethereum() {
        let mut v = setup();
        ctx(agent(), 2);
        v.enable_eth(1, agent(), U128(1), U128(1), U128(1), U128(1), 0);
    }

    #[test]
    fn disabling_ethereum_blocks_new_payments() {
        let mut v = with_eth();
        v.disable_eth();
        ctx(agent(), 2);
        let r = v.request_spend(eth_shop(), U128(1), "x".into(), eth());
        assert_eq!((r.status, r.rule.as_str()), (Status::Blocked, "chain_not_enabled"));
    }

    fn armed(cooldown_secs: Option<u64>) -> Leash {
        let mut v = setup();
        v.set_tripwire(3, 600, cooldown_secs);
        v
    }

    const SEC: u64 = 1_000_000_000;

    #[test]
    fn tripwire_freezes_after_repeated_attacks() {
        let mut v = armed(None);
        ctx(agent(), 10 * SEC);
        assert_eq!(v.request_spend(attacker(), near(1), "a".into(), None).rule, "receiver_blocklisted: known drainer");
        v.request_spend(attacker(), near(1), "b".into(), None);
        assert_eq!(v.get_vault().recent_blocked, 2);
        let third = v.request_spend(attacker(), near(1), "c".into(), None);
        assert!(third.rule.ends_with("; tripwire_tripped"), "{}", third.rule);
        let view = v.get_vault();
        assert!(view.frozen && !view.frozen_by_owner);
        assert_eq!(view.tripped_until, Some(u64::MAX));
        assert_eq!(view.stats.tripwire_trips, 1);
        // Even a normal coffee is refused now, and freezes do not count as new attacks.
        let coffee = v.request_spend(cafe(), U128(1), "coffee".into(), None);
        assert_eq!((coffee.status, coffee.rule.as_str()), (Status::Blocked, "tripwire_frozen"));
        assert_eq!(v.check_spend(cafe(), U128(1), None).rule, "tripwire_frozen");
        assert_eq!(v.get_vault().stats.tripwire_trips, 1);
    }

    #[test]
    fn tripwire_without_cooldown_needs_the_owner() {
        let mut v = armed(None);
        ctx(agent(), 10 * SEC);
        for _ in 0..3 {
            v.request_spend(attacker(), near(1), "x".into(), None);
        }
        ctx(agent(), 10 * SEC + 30 * DAY_NS);
        assert_eq!(v.request_spend(cafe(), U128(1), "later".into(), None).rule, "tripwire_frozen");
        ctx(owner(), 11 * SEC + 30 * DAY_NS);
        v.unfreeze();
        ctx(agent(), 12 * SEC + 30 * DAY_NS);
        assert_eq!(v.request_spend(cafe(), U128(1), "after unfreeze".into(), None).status, Status::Executed);
    }

    #[test]
    fn tripwire_cooldown_lifts_by_itself() {
        let mut v = armed(Some(120));
        ctx(agent(), 10 * SEC);
        for _ in 0..3 {
            v.request_spend(attacker(), near(1), "x".into(), None);
        }
        ctx(agent(), 129 * SEC);
        assert_eq!(v.request_spend(cafe(), U128(1), "too soon".into(), None).rule, "tripwire_frozen");
        ctx(agent(), 131 * SEC);
        assert_eq!(v.get_vault().tripped_until, None);
        assert_eq!(v.request_spend(cafe(), U128(1), "after cooldown".into(), None).status, Status::Executed);
    }

    #[test]
    fn tripwire_forgets_blocks_outside_its_window() {
        let mut v = armed(None);
        ctx(agent(), 10 * SEC);
        v.request_spend(attacker(), near(1), "a".into(), None);
        v.request_spend(attacker(), near(1), "b".into(), None);
        ctx(agent(), 700 * SEC);
        assert_eq!(v.get_vault().recent_blocked, 0);
        let r = v.request_spend(attacker(), near(1), "c".into(), None);
        assert!(!r.rule.contains("tripwire"));
        assert!(!v.get_vault().frozen);
    }

    #[test]
    fn no_tripwire_means_no_auto_freeze() {
        let mut v = setup();
        ctx(agent(), 10 * SEC);
        for _ in 0..10 {
            v.request_spend(attacker(), near(1), "x".into(), None);
        }
        assert!(!v.get_vault().frozen);
    }

    #[test]
    #[should_panic(expected = "only_owner")]
    fn agent_cannot_disarm_the_tripwire() {
        let mut v = armed(None);
        ctx(agent(), 2);
        v.disable_tripwire();
    }

    #[test]
    #[should_panic(expected = "max_blocked_out_of_range")]
    fn tripwire_rejects_a_zero_threshold() {
        let mut v = setup();
        v.set_tripwire(0, 600, None);
    }

    #[test]
    fn stats_count_every_outcome() {
        let mut v = setup();
        ctx(agent(), 2);
        v.request_spend(cafe(), near(1), "paid".into(), None);
        v.request_spend(attacker(), near(2), "blocked".into(), None);
        let a = v.request_spend(cafe(), near(2), "held then approved".into(), None);
        let b = v.request_spend("stranger.testnet".parse().unwrap(), near(1), "held then denied".into(), None);
        ctx(owner(), 3);
        v.approve(a.id);
        v.deny(b.id);
        let s = v.get_vault().stats;
        assert_eq!((s.requests, s.paid, s.held, s.denied, s.blocked), (4, 2, 2, 1, 1));
        assert_eq!(s.near_paid, near(3));
        assert_eq!(s.near_blocked, near(2));
        assert_eq!(s.near_denied, near(1));
    }

    #[test]
    fn newest_requests_first() {
        let mut v = setup();
        ctx(agent(), 2);
        for i in 0..5 {
            v.request_spend(cafe(), U128(1 + i), "x".into(), None);
        }
        let page = v.get_requests(None, Some(2));
        assert_eq!(page.iter().map(|r| r.request.id).collect::<Vec<_>>(), vec![4, 3]);
        let next = v.get_requests(Some(3), Some(2));
        assert_eq!(next.iter().map(|r| r.request.id).collect::<Vec<_>>(), vec![2, 1]);
    }
}
