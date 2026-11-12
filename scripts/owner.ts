/**
 * Owner CLI. Signs with ROOT_PRIVATE_KEY from .env.local, which is the vault
 * owner until you hand ownership to a wallet with set-owner.
 *
 *   npm run owner -- status
 *   npm run owner -- approve <id> | deny <id> | deny-all
 *   npm run owner -- freeze | unfreeze
 *   npm run owner -- allow <account> | block <account> <reason...>
 *   npm run owner -- set-owner <account>
 *   npm run owner -- recycle      # sweep demo merchants' NEAR back into the vault
 *   npm run owner -- caps <per-payment> <daily>   # e.g. caps 0.5 3 (NEAR)
 *   npm run owner -- reset        # start a fresh daily window now
 *   npm run owner -- add-reset-key  # function-call key that can only call reset_window()
 *   npm run owner -- upgrade [migrate]  # redeploy contract/target/near/leash_vault.wasm;
 *                                       # "migrate" also converts v1.0 state in the same tx
 *   npm run owner -- limit <account> <near>  # per-receiver daily limit, e.g. limit books 1.2
 *   npm run owner -- unlimit <account>
 *   npm run owner -- check <account> <near>  # free dry run: what would request_spend do?
 *   npm run owner -- tripwire <refusals> <minutes> [cooldown-minutes]  # e.g. tripwire 3 10 2
 *   npm run owner -- tripwire off
 *   npm run owner -- eth-enable <per-payment ETH> <daily ETH> [max-fee-gwei] [tip-gwei]
 *                     # cross-chain via Chain Signatures on Sepolia; reads the nonce from Sepolia,
 *                     # allowlists the demo shop and blocklists the demo drainer
 *   npm run owner -- eth-off
 *   npm run owner -- eth-address    # the vault's Ethereum address and balance
 *   npm run owner -- eth-send <id>  # (re)broadcast a signed Ethereum request
 */
import { config } from "dotenv";
config({ path: ".env.local" });
import { readFileSync, appendFileSync } from "node:fs";
import { Account, KeyPair, KeyPairSigner, actions, nearToYocto, teraToGas } from "near-api-js";
import type { KeyPairString } from "near-api-js";

const { getProvider, viewVault, viewRequests, viewCheckSpend, resolveAccount, fmtNear, VAULT_ID } = await import("../src/lib/chain");
const { ruleLabel } = await import("../src/lib/rules");
const eth = await import("../src/lib/eth");
const { parseEther, parseGwei } = await import("viem");

const rootId = process.env.ROOT_ACCOUNT_ID!;
const signer = KeyPairSigner.fromSecretKey(process.env.ROOT_PRIVATE_KEY as KeyPairString);
const root = new Account(rootId, getProvider(), signer);
const call = (methodName: string, args: Record<string, unknown> = {}) =>
  root.callFunction({ contractId: VAULT_ID, methodName, args, gas: teraToGas("60") });

const [cmd, ...rest] = process.argv.slice(2);

switch (cmd) {
  case "status":
  case undefined: {
    const v = await viewVault();
    console.log(`${VAULT_ID}  owner=${v.owner}  agent=${v.agent}  frozen=${v.frozen}`);
    console.log(`available ${fmtNear(v.available, 3)} NEAR, spent today ${fmtNear(v.spent_today, 3)} / ${fmtNear(v.daily_cap)} NEAR`);
    for (const c of v.receiver_caps ?? []) {
      console.log(`limit ${c.receiver}: ${fmtNear(c.spent_today, 3)} / ${fmtNear(c.daily_cap)} NEAR today`);
    }
    if (v.tripwire) {
      const t = v.tripwire;
      console.log(
        `tripwire: ${t.max_blocked} refusals in ${t.window_secs / 60} min, ${t.cooldown_secs ? `cooldown ${t.cooldown_secs / 60} min` : "owner must unfreeze"}; ${v.recent_blocked ?? 0} counted now${v.tripped_until ? ", TRIPPED" : ""}`,
      );
    }
    if (v.eth) {
      const e = v.eth;
      const addr = await eth.deriveEthAddress();
      const bal = await eth.ethBalance(addr).catch(() => null);
      console.log(
        `ethereum (${eth.ETH_CHAIN_NAME}): ${addr}, balance ${bal === null ? "?" : eth.fmtEth(bal)} ETH; ${eth.fmtEth(v.eth_spent_today ?? "0")} / ${eth.fmtEth(e.daily_cap)} ETH today, ${eth.fmtEth(e.per_tx_cap)} per payment, next nonce ${e.next_nonce}`,
      );
    }
    if (v.stats) {
      const s = v.stats;
      console.log(`stats: ${s.requests} requests, ${s.paid} paid (${fmtNear(s.near_paid, 2)} NEAR), ${s.blocked} blocked, ${s.denied} denied, ${s.tripwire_trips} tripwire freezes`);
    }
    for (const r of await viewRequests(15)) {
      const amount = r.chain === "ethereum" ? `${eth.fmtEth(r.amount).padStart(7)} ETH ` : `${fmtNear(r.amount, 3).padStart(7)} NEAR`;
      console.log(`#${r.id}  ${r.status.padEnd(8)} ${amount} -> ${r.receiver}  (${ruleLabel(r.rule)})`);
    }
    break;
  }
  case "approve":
  case "deny": {
    const id = Number(rest[0]);
    const isEth = (await eth.isEthRequestId(id));
    // Approving an Ethereum request also asks the MPC network to sign it.
    const r = await root.callFunction({ contractId: VAULT_ID, methodName: cmd, args: { id }, gas: teraToGas(isEth ? "150" : "60") });
    console.log(r);
    if (cmd === "approve" && isEth) {
      const out = await eth.settleEthRequest(id);
      console.log(out.stage === "broadcast" ? `sent on ${eth.ETH_CHAIN_NAME}: ${eth.ethExplorerTx(out.hash)}` : out.message);
    }
    break;
  }
  case "eth-enable": {
    const [perTx, daily, maxFee = "20", tip = "1.5"] = rest;
    if (!perTx || !daily) {
      console.log("usage: eth-enable <per-payment ETH> <daily ETH> [max-fee-gwei] [tip-gwei]");
      break;
    }
    const addr = await eth.deriveEthAddress();
    const nonce = await eth.ethNonce(addr);
    await call("enable_eth", {
      chain_id: eth.ETH_CHAIN_ID,
      mpc_contract: eth.MPC_CONTRACT,
      per_tx_cap: parseEther(perTx).toString(),
      daily_cap: parseEther(daily).toString(),
      max_fee_per_gas: parseGwei(maxFee).toString(),
      max_priority_fee_per_gas: parseGwei(tip).toString(),
      next_nonce: nonce,
    });
    const { shop, drainer } = await eth.demoEthAccounts();
    await call("allow", { account: shop.toLowerCase() });
    await call("block", { account: drainer.toLowerCase(), reason: "reported drainer address" });
    console.log(`ethereum on: ${addr} (nonce ${nonce}), ${perTx} ETH per payment, ${daily} ETH a day, fees up to ${maxFee} gwei`);
    console.log(`allowlisted shop ${shop}, blocklisted drainer ${drainer}`);
    break;
  }
  case "eth-off":
    await call("disable_eth");
    console.log("ethereum off: new cross-chain requests are blocked");
    break;
  case "eth-address": {
    const addr = await eth.deriveEthAddress();
    const bal = await eth.ethBalance(addr).catch(() => null);
    console.log(`${addr}  ${bal === null ? "?" : eth.fmtEth(bal)} ETH on ${eth.ETH_CHAIN_NAME}  ${eth.ethExplorerAddress(addr)}`);
    break;
  }
  case "eth-send": {
    const out = await eth.settleEthRequest(Number(rest[0]));
    console.log(out.stage === "broadcast" ? `sent: ${eth.ethExplorerTx(out.hash)}` : out.message);
    break;
  }
  case "deny-all": {
    const pending = (await viewRequests(100)).filter((r) => r.status === "Pending");
    for (const r of pending) {
      await call("deny", { id: r.id });
      console.log(`denied #${r.id}`);
    }
    if (!pending.length) console.log("nothing pending");
    break;
  }
  case "freeze":
  case "unfreeze":
    await call(cmd);
    console.log(`${cmd} done`);
    break;
  case "allow":
    await call("allow", { account: rest[0] });
    console.log(`allowed ${rest[0]}`);
    break;
  case "block":
    await call("block", { account: rest[0], reason: rest.slice(1).join(" ") || "blocked by owner" });
    console.log(`blocked ${rest[0]}`);
    break;
  case "set-owner":
    await call("set_owner", { owner: rest[0] });
    console.log(`owner is now ${rest[0]}`);
    break;
  case "recycle": {
    for (const prefix of ["cafe", "books"]) {
      const id = `${prefix}.${rootId}`;
      const acct = new Account(id, getProvider(), signer);
      const { balance } = await acct.getState();
      const spare = balance.available - nearToYocto("0.05");
      if (spare > 0n) {
        await acct.transfer({ receiverId: VAULT_ID, amount: spare });
        console.log(`moved ${fmtNear(spare, 3)} NEAR from ${prefix} back to the vault`);
      }
    }
    break;
  }
  case "caps": {
    const [perTx, daily] = rest;
    if (!perTx || !daily) {
      console.log("usage: caps <per-payment NEAR> <daily NEAR>");
      break;
    }
    await call("set_caps", { per_tx_cap: nearToYocto(perTx as `${number}`).toString(), daily_cap: nearToYocto(daily as `${number}`).toString() });
    const v = await viewVault();
    console.log(`caps now ${fmtNear(v.per_tx_cap)} NEAR per payment, ${fmtNear(v.daily_cap)} NEAR a day`);
    break;
  }
  case "reset":
    await call("reset_window");
    console.log("daily window reset");
    break;
  case "add-reset-key": {
    // Same pattern as the agent: a key on the owner account that can call
    // exactly one vault method. The hosted demo uses it so visitors do not
    // exhaust each other's daily allowance. Stored in .env.local, never printed.
    if (/^DEMO_RESET_PRIVATE_KEY=./m.test(readFileSync(".env.local", "utf8"))) {
      console.log("DEMO_RESET_PRIVATE_KEY already exists in .env.local");
      break;
    }
    const kp = KeyPair.fromRandom("ed25519");
    await root.signAndSendTransaction({
      receiverId: rootId,
      actions: [actions.addFunctionCallAccessKey(kp.getPublicKey(), VAULT_ID, ["reset_window"], nearToYocto("0.25"))],
    });
    appendFileSync(".env.local", `
DEMO_RESET_ACCOUNT_ID=${rootId}
DEMO_RESET_PRIVATE_KEY=${kp.toString()}
`);
    console.log(`added reset-only key ${kp.getPublicKey().toString()} on ${rootId}; saved to .env.local`);
    break;
  }
  case "upgrade": {
    // Deploy and migrate in one transaction: if migrate fails, the deploy is
    // reverted too, so the vault never runs new code on old state.
    const wasm = readFileSync("contract/target/near/leash_vault.wasm");
    const vault = new Account(VAULT_ID, getProvider(), signer);
    const steps = [actions.deployContract(new Uint8Array(wasm))];
    if (rest[0] === "migrate") steps.push(actions.functionCall("migrate", {}, teraToGas("100"), 0n));
    await vault.signAndSendTransaction({ receiverId: VAULT_ID, actions: steps });
    console.log(`deployed ${wasm.length} bytes to ${VAULT_ID}${rest[0] === "migrate" ? " and migrated state" : ""}`);
    break;
  }
  case "limit": {
    const [who, amount] = rest;
    if (!who || !amount) {
      console.log("usage: limit <account> <daily NEAR>");
      break;
    }
    const receiver = resolveAccount(who);
    await call("set_receiver_cap", { receiver, daily_cap: nearToYocto(amount as `${number}`).toString() });
    console.log(`${receiver} can now get at most ${amount} NEAR a day without the owner`);
    break;
  }
  case "unlimit":
    await call("remove_receiver_cap", { receiver: resolveAccount(rest[0]) });
    console.log(`removed the limit on ${resolveAccount(rest[0])}`);
    break;
  case "tripwire": {
    if (rest[0] === "off") {
      await call("disable_tripwire");
      console.log("tripwire disabled");
      break;
    }
    const [count, mins, cool] = rest.map(Number);
    if (!count || !mins) {
      console.log("usage: tripwire <refusals> <minutes> [cooldown-minutes] | tripwire off");
      break;
    }
    await call("set_tripwire", { max_blocked: count, window_secs: mins * 60, cooldown_secs: cool ? cool * 60 : null });
    console.log(`tripwire: ${count} refusals in ${mins} min freeze the vault${cool ? ` for ${cool} min` : " until the owner unfreezes it"}`);
    break;
  }
  case "check": {
    const receiver = resolveAccount(rest[0] ?? "");
    const d = await viewCheckSpend(receiver, nearToYocto((rest[1] ?? "0") as `${number}`));
    console.log(`${rest[1]} NEAR -> ${receiver}: ${d.status} (${ruleLabel(d.rule)})`);
    break;
  }
  default:
    console.log("unknown command; see the header of scripts/owner.ts");
}
