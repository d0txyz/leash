# Leash on EVM

The Leash spending policy as a Solidity contract, for Base and other EVM chains.

An AI agent gets one address that can call one function, `requestSpend`. The vault holds one ERC-20 token (USDC in practice) and decides every payment on-chain. A jailbroken or prompt-injected agent still cannot move money outside the policy, because the policy is not in the agent.

Status: in development. Not deployed, not audited. The NEAR contract in `../contract` is the version that is live on testnet.

## The policy

`requestSpend(receiver, amount, reason)` runs these rules in order:

| # | Check | Result |
| --- | --- | --- |
| 1 | Vault frozen by the owner | Blocked (`VaultFrozen`) |
| 2 | Vault frozen by the tripwire | Blocked (`TripwireFrozen`) |
| 3 | Receiver on the blocklist | Blocked (`ReceiverBlocklisted`) |
| 4 | Amount above the vault balance | Blocked (`InsufficientFunds`) |
| 5 | Amount above the per-payment cap | Pending (`OverPerTxCap`) |
| 6 | Would exceed the daily cap | Pending (`OverDailyCap`) |
| 7 | Would exceed that receiver's own daily limit | Pending (`OverReceiverCap`) |
| 8 | Receiver not on the allowlist | Pending (`NewReceiver`) |
| 9 | Otherwise | Paid at once (`WithinPolicy`) |

A pending request waits for the owner to `approve` or `deny` it. At most 25 requests wait at a time; when the queue is full, a request that would wait is blocked instead (status `Blocked` with one of the four hold rules).

A refusal never reverts. Every request is stored with its status and rule, so the history can be read back with `getRequests`. `checkSpend` runs the same rules as a view call, so an agent can ask first.

**Tripwire.** `setTripwire(maxBlocked, windowSecs, cooldownSecs)`: after `maxBlocked` blocked requests inside `windowSecs`, the vault freezes itself. With a cooldown it thaws by itself; with zero, only the owner can unfreeze. Requests refused while the vault is already frozen do not count toward the next trip.

## Roles

| Role | Can do |
| --- | --- |
| Agent | `requestSpend` only |
| Owner | Approve, deny, freeze, unfreeze, set caps, lists, receiver limits and tripwire, rotate the agent, reset the daily window, withdraw, rescue stray tokens |
| Anyone | Read everything, `checkSpend` |

Ownership moves in two steps (`transferOwnership`, then `acceptOwnership`) and cannot be renounced.

## Files

| File | What | nSLOC |
| --- | --- | --- |
| `contracts/LeashVault.sol` | The vault and the whole policy | 416 |
| `contracts/LeashFactory.sol` | Deploys a vault owned by the caller and lists a wallet's vaults | 18 |
| `test/LeashVault.t.sol` | 53 unit tests | |
| `test/LeashVault.fuzz.t.sol` | 4 fuzz tests, including a reference model of the tripwire | |
| `test/LeashVault.invariant.t.sol` | 4 invariants under random agent and owner actions | |
| `test/LeashFactory.t.sol` | 4 factory tests | |

Dependencies: OpenZeppelin Contracts 5.6.1 (`Ownable2Step`, `ReentrancyGuard`, `SafeERC20`, `EnumerableSet`, `EnumerableMap`). Solidity 0.8.28, EVM target cancun.

## Run the tests

Needs Node 22.13 or newer.

```bash
npm install
```

```bash
npm test
```

The tests are forge-std tests run by Hardhat 3. A `foundry.toml` is included for reviewers who use Foundry (`forge test` after `npm install`); it has not been run by us.

## What the tests check

- Every rule in the table, in order, plus the full approval queue.
- The agent cannot call any of the 19 owner functions.
- The dry run always agrees with the real call (fuzzed).
- Acting alone, the agent never moves more than the daily cap in one window, never more than the per-payment cap at once, and never pays a receiver that is not on the allowlist (fuzzed, and held as an invariant under random owner actions).
- A blocklisted receiver is never paid, even through an approval (invariant).
- Tokens are conserved and the counters match the ledger (invariants).
- The ring-buffer tripwire matches a plain list model step for step (fuzzed).
- A token that tries to re-enter during a payment is stopped, and a transfer that returns false reverts the request.

## Design notes and known limits

These are choices, listed here so a reviewer does not have to rediscover them.

- **Fixed daily window, not a sliding one.** The window starts at the first request after the previous one expired, and lasts 24 hours. An agent can spend up to the daily cap at the end of one window and again at the start of the next.
- **Standard ERC-20 only.** No fee on transfer, no rebasing, no transfer hooks. The token is fixed at deployment.
- **A failed transfer reverts.** If the token refuses the transfer (for example a USDC-blacklisted receiver), the request reverts and nothing is recorded. This is the one case where a request leaves no trace.
- **Approvals can exceed the caps.** That is their purpose. An approved payment still counts toward the daily totals.
- **Pending requests do not reserve funds and do not expire.** `approve` reverts if the vault cannot cover the payment, if the vault is frozen, or if the receiver was blocklisted in the meantime.
- **The owner is fully trusted.** The owner can withdraw everything. Nothing stops an owner from naming themselves as the agent, which would make the policy pointless.
- **A compromised agent can freeze the vault** by sending requests that get blocked until the tripwire trips. That is the intended outcome.
- **The request ledger grows forever.** The agent pays the gas for each entry. Views are paged (100 per call).
- **`reason` is emitted, not stored.** It is limited to 280 bytes.

## Differences from the NEAR contract

| NEAR (`contract/src/lib.rs`) | EVM |
| --- | --- |
| Pays native NEAR | Pays one ERC-20 |
| Agent is a function-call access key | Agent is an address |
| Transfer is async; a bounce becomes status `Failed` | Transfer is in the same call; a failure reverts |
| Rules are strings | Rules are an enum |
| Recent blocks kept in a list | Ring buffer of 20 timestamps |
| Receiver totals keyed by window start time | Keyed by a window id, so two resets in one block cannot leave a stale total |
| `approve` does not look at the blocklist | `approve` reverts for a blocklisted receiver |
| Single-step `set_owner` | Two-step ownership, no renounce |
| Cross-chain payments through Chain Signatures | Not applicable |
