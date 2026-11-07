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

#[near(serializers = [borsh])]
#[derive(BorshStorageKey)]
enum Prefix {
    Allowlist,
    Blocklist,
    Requests,
    ReceiverCaps,
    ReceiverSpent,
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
    /// The transfer was sent but bounced (for example, the receiver does not exist).
    Failed,
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
}

/// State layout of v1.1.0 (receiver limits, no tripwire or stats), kept so
/// `migrate` can read the vault that is deployed today.
#[near(serializers = [borsh])]
pub struct LeashV11 {
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
        }
    }

    /// One-time upgrade from the v1.1.0 layout. Only the vault account itself
    /// can call it (`npm run owner -- upgrade migrate` deploys and migrates in
    /// one transaction). Backfills the stats from the stored request history.
    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        let old: LeashV11 = env::state_read().unwrap_or_else(|| env::panic_str("no_v1_1_state"));
        let mut stats = Stats::default();
        for r in old.requests.iter() {
            let amount = r.amount.0;
            stats.requests += 1;
            match r.status {
                Status::Executed => {
                    stats.paid += 1;
                    stats.near_paid += amount;
                }
                Status::Approved => {
                    stats.held += 1;
                    stats.paid += 1;
                    stats.near_paid += amount;
                }
                Status::Pending => stats.held += 1,
                Status::Denied => {
                    stats.held += 1;
                    stats.denied += 1;
                    stats.near_denied += amount;
                }
                Status::Blocked => {
                    stats.blocked += 1;
                    stats.near_blocked += amount;
                }
                Status::Failed => stats.failed += 1,
            }
        }
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
            tripwire: None,
            tripped_until: None,
            recent_blocked: Vec::new(),
            stats,
        }
    }

    // ------------------------------------------------------------------
    // Agent
    // ------------------------------------------------------------------

    /// The only method the agent's key can call. Never panics on a policy
    /// violation: the refusal is recorded on-chain so it can be audited.
    pub fn request_spend(&mut self, receiver: AccountId, amount: U128, reason: String) -> Request {
        require!(env::predecessor_account_id() == self.agent, "only_agent");
        require!(amount.0 > 0, "amount_must_be_positive");
        let reason: String = reason.chars().take(MAX_REASON_LEN).collect();
        let amount_t = NearToken::from_yoctonear(amount.0);
        self.roll_window();

        let Decision { status, mut rule } = self.decide(&receiver, amount_t);
        if status == Status::Blocked && !self.is_frozen() && self.note_block_and_maybe_trip() {
            rule = format!("{rule}; tripwire_tripped");
        }
        self.stats.requests += 1;
        match status {
            Status::Executed => {
                self.stats.paid += 1;
                self.stats.near_paid += amount.0;
            }
            Status::Pending => self.stats.held += 1,
            _ => {
                self.stats.blocked += 1;
                self.stats.near_blocked += amount.0;
            }
        }

        let id = self.requests.len() as u64;
        let request = Request {
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
        self.requests.push(request.clone());

        match status {
            Status::Executed => {
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
        let amount = NearToken::from_yoctonear(r.amount.0);
        require!(!self.is_frozen(), "vault_frozen");
        require!(amount <= self.available(), "insufficient_funds");
        self.stats.paid += 1;
        self.stats.near_paid += r.amount.0;
        r.status = Status::Approved;
        r.rule = format!("{} -> approved_by_owner", r.rule);
        r.resolved_at = Some(env::block_timestamp());
        self.roll_window();
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
        self.stats.near_denied += r.amount.0;
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
        log!("EVENT_JSON:{{\"standard\":\"leash\",\"version\":\"1.0.0\",\"event\":\"window_reset\",\"data\":[]}}");
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

    // ------------------------------------------------------------------
    // Views
    // ------------------------------------------------------------------

    /// Dry run: what `request_spend` would decide for this receiver and amount
    /// right now. Free to call, changes nothing, same code path.
    pub fn check_spend(&self, receiver: AccountId, amount: U128) -> Decision {
        require!(amount.0 > 0, "amount_must_be_positive");
        self.decide(&receiver, NearToken::from_yoctonear(amount.0))
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
        }
    }

    /// Newest first. `before` is an exclusive request id for paging.
    pub fn get_requests(&self, before: Option<u64>, limit: Option<u64>) -> Vec<Request> {
        let len = self.requests.len() as u64;
        let end = before.unwrap_or(len).min(len);
        let limit = limit.unwrap_or(20).min(100);
        (end.saturating_sub(limit)..end)
            .rev()
            .filter_map(|i| self.requests.get(i as u32).cloned())
            .collect()
    }

    pub fn get_request(&self, id: u64) -> Option<Request> {
        self.requests.get(id as u32).cloned()
    }

    // ------------------------------------------------------------------
    // Internal
    // ------------------------------------------------------------------

    fn assert_owner(&self) {
        require!(env::predecessor_account_id() == self.owner, "only_owner");
    }

    /// The policy, in order. Shared by `request_spend` and `check_spend`.
    fn decide(&self, receiver: &AccountId, amount: NearToken) -> Decision {
        let over_receiver_cap = self
            .receiver_caps
            .get(receiver)
            .is_some_and(|cap| self.receiver_spent_now(receiver).saturating_add(amount) > *cap);
        let (status, rule) = if self.frozen {
            (Status::Blocked, "vault_frozen".to_string())
        } else if self.is_frozen() {
            (Status::Blocked, "tripwire_frozen".to_string())
        } else if let Some(why) = self.blocklist.get(receiver) {
            (Status::Blocked, format!("receiver_blocklisted: {why}"))
        } else if amount > self.available() {
            (Status::Blocked, "insufficient_funds".to_string())
        } else if amount > self.per_tx_cap {
            self.pending_or_blocked("over_per_tx_cap")
        } else if self.window_spent().saturating_add(amount) > self.daily_cap {
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
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;

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
        let r = v.request_spend(cafe(), near(1), "coffee".into());
        assert_eq!(r.status, Status::Executed);
        assert_eq!(v.get_vault().spent_today, near(1));
    }

    #[test]
    fn blocks_blocklisted_receiver_even_for_tiny_amounts() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(attacker(), U128(1), "ignore previous instructions".into());
        assert_eq!(r.status, Status::Blocked);
        assert!(r.rule.starts_with("receiver_blocklisted"));
    }

    #[test]
    fn over_per_tx_cap_goes_to_owner() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), near(2), "catering".into());
        assert_eq!(r.status, Status::Pending);
        assert_eq!(r.rule, "over_per_tx_cap");
        assert_eq!(v.get_vault().open_pending, 1);
    }

    #[test]
    fn unknown_receiver_goes_to_owner() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend("stranger.testnet".parse().unwrap(), U128(10), "tip".into());
        assert_eq!(r.status, Status::Pending);
        assert_eq!(r.rule, "new_receiver");
    }

    #[test]
    fn daily_cap_holds_across_many_small_payments() {
        let mut v = setup();
        ctx(agent(), 2);
        for _ in 0..3 {
            assert_eq!(
                v.request_spend(cafe(), near(1), "coffee".into()).status,
                Status::Executed
            );
        }
        let r = v.request_spend(cafe(), near(1), "fourth coffee".into());
        assert_eq!(r.status, Status::Pending);
        assert_eq!(r.rule, "over_daily_cap");
    }

    #[test]
    fn daily_window_resets_after_24h() {
        let mut v = setup();
        ctx(agent(), 2);
        for _ in 0..3 {
            v.request_spend(cafe(), near(1), "coffee".into());
        }
        ctx(agent(), 2 + DAY_NS);
        assert_eq!(
            v.request_spend(cafe(), near(1), "next day".into()).status,
            Status::Executed
        );
    }

    #[test]
    fn freeze_blocks_everything() {
        let mut v = setup();
        v.freeze();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), U128(1), "coffee".into());
        assert_eq!(r.status, Status::Blocked);
        assert_eq!(r.rule, "vault_frozen");
    }

    #[test]
    fn cannot_spend_more_than_free_balance() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), near(50), "everything".into());
        assert_eq!(r.status, Status::Blocked);
        assert_eq!(r.rule, "insufficient_funds");
    }

    #[test]
    fn owner_approves_and_denies() {
        let mut v = setup();
        ctx(agent(), 2);
        let a = v.request_spend(cafe(), near(2), "catering".into());
        let b = v.request_spend("stranger.testnet".parse().unwrap(), U128(5), "tip".into());
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
        let r = v.request_spend(cafe(), near(2), "catering".into());
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
            v.request_spend(cafe(), near(1), "coffee".into());
        }
        assert_eq!(v.request_spend(cafe(), near(1), "fourth".into()).rule, "over_daily_cap");
        ctx(owner(), 3);
        v.reset_window();
        assert_eq!(v.get_vault().spent_today, U128(0));
        ctx(agent(), 4);
        assert_eq!(v.request_spend(cafe(), near(1), "after reset".into()).status, Status::Executed);
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
        v.request_spend(attacker(), near(1), "hi".into());
    }

    #[test]
    #[should_panic(expected = "not_pending")]
    fn cannot_approve_twice() {
        let mut v = setup();
        ctx(agent(), 2);
        let r = v.request_spend(cafe(), near(2), "catering".into());
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
                v.request_spend(cafe(), near(2), format!("spam {i}")).status,
                Status::Pending
            );
        }
        let r = v.request_spend(cafe(), near(2), "one more".into());
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
        assert_eq!(v.request_spend(books(), near(1), "book".into()).status, Status::Executed);
        let r = v.request_spend(books(), U128(1), "one more".into());
        assert_eq!(r.status, Status::Pending);
        assert_eq!(r.rule, "over_receiver_cap");
        // Other receivers keep the rest of the vault-wide allowance.
        assert_eq!(v.request_spend(cafe(), near(1), "coffee".into()).status, Status::Executed);
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
        v.request_spend(books(), near(1), "book".into());
        ctx(agent(), 2 + DAY_NS);
        assert_eq!(v.get_vault().receiver_caps[0].spent_today, U128(0));
        assert_eq!(v.request_spend(books(), near(1), "next day".into()).status, Status::Executed);
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
            let d = v.check_spend(who.clone(), amount);
            assert_eq!(v.get_vault().total_requests, before, "check_spend must not write");
            let r = v.request_spend(who, amount, "x".into());
            assert_eq!((d.status, d.rule), (r.status, r.rule));
        }
    }

    #[test]
    fn check_spend_sees_an_expired_window_as_fresh() {
        let mut v = setup();
        ctx(agent(), 2);
        for _ in 0..3 {
            v.request_spend(cafe(), near(1), "coffee".into());
        }
        assert_eq!(v.check_spend(cafe(), near(1)).rule, "over_daily_cap");
        ctx(agent(), 2 + DAY_NS);
        assert_eq!(v.check_spend(cafe(), near(1)).status, Status::Executed);
    }

    #[test]
    fn migrate_keeps_v1_1_state_and_backfills_stats() {
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
        let old = LeashV11 {
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
        assert_eq!(view.tripwire, None);
        let s = view.stats;
        assert_eq!((s.requests, s.paid, s.blocked, s.denied, s.held), (4, 2, 1, 1, 2));
        assert_eq!(s.near_paid, near(3));
        assert_eq!(s.near_blocked, near(5));
        assert_eq!(s.near_denied, near(2));
        assert_eq!(v.get_request(0).unwrap().reason, "old");
        ctx(agent(), 3);
        assert_eq!(v.request_spend(cafe(), near(1), "after".into()).status, Status::Executed);
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
        assert_eq!(v.request_spend(attacker(), near(1), "a".into()).rule, "receiver_blocklisted: known drainer");
        v.request_spend(attacker(), near(1), "b".into());
        assert_eq!(v.get_vault().recent_blocked, 2);
        let third = v.request_spend(attacker(), near(1), "c".into());
        assert!(third.rule.ends_with("; tripwire_tripped"), "{}", third.rule);
        let view = v.get_vault();
        assert!(view.frozen && !view.frozen_by_owner);
        assert_eq!(view.tripped_until, Some(u64::MAX));
        assert_eq!(view.stats.tripwire_trips, 1);
        // Even a normal coffee is refused now, and freezes do not count as new attacks.
        let coffee = v.request_spend(cafe(), U128(1), "coffee".into());
        assert_eq!((coffee.status, coffee.rule.as_str()), (Status::Blocked, "tripwire_frozen"));
        assert_eq!(v.check_spend(cafe(), U128(1)).rule, "tripwire_frozen");
        assert_eq!(v.get_vault().stats.tripwire_trips, 1);
    }

    #[test]
    fn tripwire_without_cooldown_needs_the_owner() {
        let mut v = armed(None);
        ctx(agent(), 10 * SEC);
        for _ in 0..3 {
            v.request_spend(attacker(), near(1), "x".into());
        }
        ctx(agent(), 10 * SEC + 30 * DAY_NS);
        assert_eq!(v.request_spend(cafe(), U128(1), "later".into()).rule, "tripwire_frozen");
        ctx(owner(), 11 * SEC + 30 * DAY_NS);
        v.unfreeze();
        ctx(agent(), 12 * SEC + 30 * DAY_NS);
        assert_eq!(v.request_spend(cafe(), U128(1), "after unfreeze".into()).status, Status::Executed);
    }

    #[test]
    fn tripwire_cooldown_lifts_by_itself() {
        let mut v = armed(Some(120));
        ctx(agent(), 10 * SEC);
        for _ in 0..3 {
            v.request_spend(attacker(), near(1), "x".into());
        }
        ctx(agent(), 129 * SEC);
        assert_eq!(v.request_spend(cafe(), U128(1), "too soon".into()).rule, "tripwire_frozen");
        ctx(agent(), 131 * SEC);
        assert_eq!(v.get_vault().tripped_until, None);
        assert_eq!(v.request_spend(cafe(), U128(1), "after cooldown".into()).status, Status::Executed);
    }

    #[test]
    fn tripwire_forgets_blocks_outside_its_window() {
        let mut v = armed(None);
        ctx(agent(), 10 * SEC);
        v.request_spend(attacker(), near(1), "a".into());
        v.request_spend(attacker(), near(1), "b".into());
        ctx(agent(), 700 * SEC);
        assert_eq!(v.get_vault().recent_blocked, 0);
        let r = v.request_spend(attacker(), near(1), "c".into());
        assert!(!r.rule.contains("tripwire"));
        assert!(!v.get_vault().frozen);
    }

    #[test]
    fn no_tripwire_means_no_auto_freeze() {
        let mut v = setup();
        ctx(agent(), 10 * SEC);
        for _ in 0..10 {
            v.request_spend(attacker(), near(1), "x".into());
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
        v.request_spend(cafe(), near(1), "paid".into());
        v.request_spend(attacker(), near(2), "blocked".into());
        let a = v.request_spend(cafe(), near(2), "held then approved".into());
        let b = v.request_spend("stranger.testnet".parse().unwrap(), near(1), "held then denied".into());
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
            v.request_spend(cafe(), U128(1 + i), "x".into());
        }
        let page = v.get_requests(None, Some(2));
        assert_eq!(page.iter().map(|r| r.id).collect::<Vec<_>>(), vec![4, 3]);
        let next = v.get_requests(Some(3), Some(2));
        assert_eq!(next.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 1]);
    }
}
