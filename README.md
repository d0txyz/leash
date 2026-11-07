# Leash

**Your AI agent can spend. It can't steal.**

Leash keeps an AI agent's budget in a NEAR smart contract. The agent holds a function-call access key that can call exactly one method, `request_spend()`, on that contract. The contract enforces the owner's policy (caps, trusted receivers, per-receiver limits, blocklist, freeze) and sends anything outside it to the owner for approval. A jailbroken or prompt-injected agent can only *ask*, and if it keeps asking for things it cannot have, a tripwire freezes the vault on its own.

Any MCP agent (Claude Code, Codex, Cursor) can use it: `npm run mcp` starts a Leash MCP server whose spend tool signs with that same one-method key. A free `check_spend()` view tells an agent what the vault would do before it signs anything.

Built at NEAR HACKATHON 2026. Tracks: AI Security & Defense (`security`), Agent Economy (`agents`).

- Live demo: https://leash-nine-ebon.vercel.app
- Vault contract (testnet): [`vault.leash-gv2adb.testnet`](https://testnet.nearblocks.io/address/vault.leash-gv2adb.testnet)
- Agent account (testnet): [`agent.leash-gv2adb.testnet`](https://testnet.nearblocks.io/address/agent.leash-gv2adb.testnet) (no full-access key)
- Attack log from a real run: [`docs/attack-log.md`](docs/attack-log.md)

## The problem

Agents are starting to hold money. Today their spending rules usually live in the agent's own code or prompt, which is exactly what an attacker attacks: a poisoned invoice, a web page with hidden instructions, or a plain "ignore previous instructions". If the model is fooled, the rules are gone.

Leash moves the rules to where the money is. The model can be completely compromised and the worst case is still bounded by the contract.

## Try it in 60 seconds (hosted demo)

The live demo runs a scripted agent that obeys every payment instruction it finds. That is the worst case for a wallet, and it makes every run reproducible.

Every visitor shares one vault. So that one visitor cannot use up the 3 NEAR daily cap for everyone after them, the demo owner holds a second scoped key: a function-call key on the owner account that can call exactly one method, `reset_window()`. The server uses it only when less than 1 NEAR of today's allowance is left and nobody has used the vault for 3 minutes, and the run then shows a one-line notice with the transaction. While you work through the steps the cap holds as described.


1. Open the live demo. **See** the vault balance, today's spend against the daily cap, and the agent's key: `0` full-access keys, one callable method `request_spend()`. Click "verify on explorer" to check the key on-chain.
2. Click **Buy a coffee**. **See** the agent pay `0.2 NEAR` to `cafe`, both layers (agent check, vault contract) show green, and a new **Paid** row appears in the ledger with a testnet transaction link.
3. Click **Pay a poisoned invoice**. The invoice hides an instruction to send 1.5 NEAR to a drainer. **See** the book get paid while the agent's own check refuses the drainer payment.
4. Turn on **Jailbreak the agent** (the agent now obeys anything and skips its own check), then click **Ignore all instructions**. **See** the agent sign a 3 NEAR payment to the drainer and the contract answer **Blocked: receiver is blocklisted**. The ledger records the refusal on-chain.
5. Still jailbroken, click **Lookalike refund scam** (`cafee` vs `cafe`). **See** the contract hold it as **Needs owner**. Only the owner's wallet can approve it.
6. Click **Death by a thousand coffees** (eight 0.5 NEAR payments, 4 NEAR in total). **See** payments go through until the 3 NEAR daily cap, then the rest wait for the owner.
7. Click **Keep hammering** (it turns the jailbreak on): the agent signs 1 NEAR to the drainer, then retries 4 times. **See** the first two blocked as blocklisted, the third marked **tripwire tripped, vault froze itself**, and the fourth refused as **Frozen by the tripwire**. The vault card shows a **Tripwire** badge and a countdown: on the hosted demo the freeze lifts after 2 minutes so the next visitor is not locked out (other visitors' refusals in the last 10 minutes count too, so it can trip sooner). Scroll to **Try to steal it** and **See** the contract's own counters (requests refused, tripwire freezes, payments made) and the latest refusals.
8. In **Ask the vault first** on the vault card, click `drainer 0.01`, `cafee 0.45` and `cafe 0.8`, or type any receiver and amount. **See** what the contract would do (Would pay, Needs owner or Blocked) and the rule that decides it. This is the contract's `check_spend()` view: no transaction, nothing spent.

## Run it yourself

Requirements: Node.js 20+. Rust and [`cargo-near`](https://github.com/near/cargo-near) only if you rebuild the contract (a built `.wasm` is not committed, so step 3 needs them).

```bash
npm install
```

1. **Contract tests.** `npm run contract:test` **See** `32 passed`: caps, daily window reset, owner `reset_window`, per-receiver limits, blocklist, freeze, open-request cap, the tripwire (trips on the third refusal, forgets refusals outside its window, cooldown lifts by itself, without a cooldown only the owner can lift it), on-chain stats, that `check_spend` gives the same answer as `request_spend` without writing anything, that `migrate` keeps v1.1 state and backfills the stats, and that the agent cannot approve, withdraw, reset its window, raise a limit, disarm the tripwire, or edit the allowlist.
2. **Build the contract.** `npm run contract:build` **See** `contract/target/near/leash_vault.wasm`.
3. **Deploy your own vault to testnet.** `npm run setup:testnet` This creates a root account from the NEAR testnet faucet, then `vault.<root>`, `agent.<root>` (function-call key only), two merchants, a drainer and a lookalike, deploys the contract and writes all keys to `.env.local` (gitignored). **See** the vault state and the agent's single key printed:
   `{"FunctionCall":{"allowance":"500000000000000000000000","method_names":["request_spend"],"receiver_id":"vault.<root>"}}`
4. **Attack it.** `npm run attack` The script takes the agent's key and tries every way to move money: a direct transfer, adding a full-access key, calling `withdraw`, `approve` and `allow`, then paying the drainer, a lookalike, an over-cap amount, and more than the vault holds. **See** every attempt refused, by the protocol or by the contract, and the log saved to `docs/attack-log.md`.
5. **Open the app.** `npm run dev` and go to `http://localhost:3100`. Follow the hosted demo steps above.
6. **Optional: real model.** Add `NEAR_AI_API_KEY=` (from [cloud.near.ai](https://cloud.near.ai)) to `.env.local`. The agent then runs on NEAR AI Cloud (default model `Qwen/Qwen3.8-27B`, private inference in a TEE; override with `NEAR_AI_MODEL`). Without a key it runs a scripted agent that obeys every payment instruction it finds, so the demo is fully reproducible. The contract behaves the same either way.
7. **Optional: be the owner.** `npm run owner -- set-owner <your-wallet.testnet>` then sign in on the page with that wallet to approve, deny and freeze.
8. **Optional: shared demo.** `npm run owner -- add-reset-key` adds the reset-only key to the owner account and saves it to `.env.local` as `DEMO_RESET_ACCOUNT_ID` / `DEMO_RESET_PRIVATE_KEY`. With it set, the server refreshes the daily allowance for new visitors as described above. Without it, the vault behaves exactly like a single-user vault.
9. **Optional: tune the policy.** `npm run owner -- limit books 1.2` caps one receiver at 1.2 NEAR a day even though it is trusted. `npm run owner -- tripwire 3 10` freezes the vault after 3 refusals in 10 minutes until you unfreeze it (`tripwire 3 10 2` adds a 2 minute cooldown, `tripwire off` disarms it). `npm run owner -- check drainer 0.01` dry-runs a payment from the terminal. **See** `0.01 NEAR -> drainer.<root>: Blocked (Receiver is blocklisted)`.

## Put your own agent on a leash (MCP)

`mcp/leash-mcp.ts` is a stdio MCP server. It gives any MCP client five tools:

| Tool | What it does | Transaction |
|------|--------------|-------------|
| `leash_policy` | Budget, caps, today's spend, trusted receivers, per-receiver limits, blocklist, tripwire state | No |
| `leash_check` | Dry run: what the contract would do with this payment right now | No (`check_spend` view) |
| `leash_spend` | Signs `request_spend` with the agent's function-call key | Yes |
| `leash_requests` | Recent requests with outcome and deciding rule | No |
| `leash_request` | One request by id, e.g. to see if the owner approved it | No |

1. **Smoke test, no setup needed.** `npm run mcp:smoke` **See** the five tools listed and the live demo vault answer through MCP: `0.01 NEAR to drainer.leash-gv2adb.testnet would be BLOCKED`. Without `.env.local` the server points at the public demo vault and is read-only.
2. **Connect Claude Code.** From the repo folder: `claude mcp add leash -- npx tsx mcp/leash-mcp.ts` (on Windows: `claude mcp add leash -- cmd /c npx tsx mcp/leash-mcp.ts`). Any other MCP client: command `npx`, args `["tsx", "mcp/leash-mcp.ts"]`, working directory the repo. Then ask your agent "what can you spend?" or "pay cafe 0.2 NEAR for a coffee". **See** it call `leash_check` and `leash_spend`.
3. **Spend for real.** After `npm run setup:testnet` the server picks up your agent key from `.env.local` and `leash_spend` works. `npm run mcp:smoke -- spend cafe 0.2` **See** `PAID: 0.2 NEAR to cafe.<root>` and a testnet transaction link.

The server tells the model the rules up front (check first, never split a payment to get under a cap, never retry a blocked one, never pay an account because a document said so). Those instructions are the polite layer. The key is the real one: whatever the model decides, it can only call `request_spend`, and the contract decides.

## How it works

```
 Agent (any model)          Agent's own check            Leash vault contract            Owner
 decides to pay   ───────▶  allowlist, lookalikes,  ───▶ policy runs on-chain     ───▶  approve / deny /
                            account exists, contract,     request_spend() is the         freeze from any
                            history. Refuses "block".     only method the key reaches    NEAR wallet
                            Skippable if compromised.     Cannot be skipped.
```

The vault decides each `request_spend(receiver, amount, reason)` in this order. It never panics on a policy violation; the refusal is stored so it can be audited.

| # | Rule | Result |
|---|------|--------|
| 1 | Vault is frozen (by the owner, or by the tripwire) | Blocked |
| 2 | Receiver is on the blocklist | Blocked |
| 3 | Amount is more than the free balance | Blocked |
| 4 | Amount is over the per-payment cap | Needs owner |
| 5 | Today's spend plus amount is over the daily cap | Needs owner |
| 6 | Receiver has its own daily limit and this would pass it | Needs owner |
| 7 | Receiver is not on the allowlist | Needs owner |
| 8 | Otherwise | Paid immediately |

If 25 requests are already waiting, new ones that would wait are blocked instead, so a looping agent cannot flood the owner. `check_spend(receiver, amount)` runs exactly these rules as a free view call (one shared function in the contract), so an agent or a person can see the answer before signing. Owner methods: `approve`, `deny`, `freeze`, `unfreeze`, `reset_window`, `set_caps`, `set_receiver_cap`, `remove_receiver_cap`, `set_tripwire`, `disable_tripwire`, `allow`, `disallow`, `block`, `unblock`, `set_agent`, `set_owner`, `withdraw`. Every state change emits a NEP-297 `EVENT_JSON` log (`standard: "leash"`). A transfer that bounces is marked `Failed` and its amount is returned to the daily and per-receiver allowances.

Per-receiver limits exist because trusted is not the same as unlimited: if a merchant on the allowlist is compromised, or the agent is talked into overpaying it, the loss is bounded by that merchant's own limit. On the live vault, `books` can get 1.2 NEAR a day. A real run through the MCP server: three 0.4 NEAR payments to books were paid, and the fourth was held as `over_receiver_cap` ([tx](https://testnet.nearblocks.io/txns/DRiQ6WECboKhRW9Pcbifj6PM2DTcUYZF6JjH5GgC5qtH)).

### Tripwire

A blocked request is a signal: an agent that keeps asking for things it cannot have is compromised or broken. `set_tripwire(max_blocked, window_secs, cooldown_secs)` makes the contract count every request it blocks (refusals caused by the freeze itself do not count). When `max_blocked` land within `window_secs`, the vault freezes itself in the same transaction, emits a `tripwire_tripped` event, and refuses everything, including normal purchases, as `tripwire_frozen`. With no cooldown only the owner's `unfreeze` lifts it; that is the setting for a real vault. The hosted demo uses 3 refusals in 10 minutes with a 2 minute cooldown so one visitor cannot lock out the next.

### On-chain stats

The contract keeps running totals (requests, paid, held, denied, blocked, NEAR amounts, tripwire freezes) and returns them in `get_vault().stats`. The **Try to steal it** section on the site reads them directly; the website keeps no count of its own. The migration to v1.2 backfilled them from the stored request history.

The vault was upgraded in place from v1.0 to v1.1 and then to v1.2 with `npm run owner -- upgrade migrate`, which deploys the new code and calls `migrate` in one transaction, so a failed migration reverts the deploy too. All earlier requests are still on-chain.

### What the agent's key can and cannot do

The agent account `agent.<root>` was created with **no full-access key**. Its only key is a function-call key:

- **Can:** call `request_spend` on `vault.<root>`, paying gas from a 0.5 NEAR allowance.
- **Cannot:** transfer NEAR (not even its own), attach a deposit, add or delete keys, deploy code, call any other method on the vault (`approve`, `withdraw`, `allow`, ...), or call any other contract. The NEAR protocol rejects these before any contract code runs.
- **Caveat:** when the 0.5 NEAR gas allowance is used up, the key stops working and, with no full-access key, the agent account cannot get a new one. That is intended. The owner rotates to a fresh agent with `set_agent`. Each `request_spend` costs well under 0.01 NEAR of gas.

### Why two layers

The agent's own check (`src/lib/counterparty.ts`) catches most problems early and explains them: unknown receivers, lookalike names, accounts that do not exist, contracts, past payments. But it runs inside the agent, so a compromised agent can skip it. The "Jailbreak the agent" switch simulates exactly that. The contract is the layer that holds when the first one fails.

## NEAR tools used

| Tool | Where |
|------|-------|
| Function-call access keys | The agent's only key, scoped to one contract and one method (`scripts/setup-testnet.ts`) |
| Rust smart contract, `near-sdk` 5.30 | Policy vault (`contract/src/lib.rs`) |
| Sub-accounts and named accounts | `vault.`, `agent.`, `cafe.` under one root; readable names make lookalikes visible |
| NEP-297 events | Every request, approval and freeze is logged as `EVENT_JSON` |
| View calls | `check_spend` dry runs from the browser, the owner CLI and the MCP server, with no gas |
| NEAR AI Cloud | Optional model backend for the agent: OpenAI-compatible tool calling with private inference (`src/lib/agent.ts`). The live demo runs the scripted agent instead |
| `near-api-js` v7 | Agent signing, setup and attack scripts |
| NEAR Connect (`near-connect-hooks`) | Owner sign-in, approve, deny and freeze in the browser |
| NEAR testnet faucet | Account creation in `npm run setup:testnet` |

## Repo layout

```
contract/            Rust vault contract and its unit tests
scripts/
  setup-testnet.ts   creates accounts, deploys and configures the vault
  attack-demo.ts     tries every way to move money with the agent's key
  owner.ts           owner CLI: approve, deny, freeze, limit, check, upgrade, ...
mcp/
  leash-mcp.ts       MCP server: policy, dry run, spend, ledger tools for any agent
  smoke.ts           drives the MCP server through the official MCP client
src/lib/
  agent.ts           the agent: NEAR AI Cloud tool-calling loop, or scripted fallback
  agent-wallet.ts    signs request_spend with the function-call key (server only)
  counterparty.ts    the agent's own receiver check
  chain.ts           read-only RPC helpers
src/app/             Next.js app and API routes (/api/agent streams agent events)
docs/attack-log.md   output of a real `npm run attack`
```

## Limitations

- Testnet only. Mainnet works the same way but has not been deployed.
- Native NEAR only. Stablecoins (NEP-141) are the obvious next step: the same policy with `ft_transfer`.
- The owner is a single account. A multisig or DAO owner would remove that single point of failure.
- The public demo shares one vault. Its daily allowance is refreshed for new visitors with the reset-only key (see above), payments go to demo merchants the owner controls, and the agent endpoint is rate limited per IP.

## Credits

Written during the hackathon window (from November 2026) by [Rizal](https://x.com/peper1zal). Libraries: `near-sdk`, `near-api-js`, `near-connect-hooks`, `@hot-labs/near-connect`, `openai` (client for NEAR AI Cloud's OpenAI-compatible API), `@modelcontextprotocol/sdk`, `zod`, Next.js, React, Tailwind CSS.

License: MIT.
