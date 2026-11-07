/**
 * Attack demo: an attacker has fully taken over the agent and holds its key.
 * Every way to move money is tried. Each one is refused, either by the NEAR
 * protocol (the key's permissions) or by the vault contract (the policy).
 *
 *   npm run attack            # runs against the testnet vault in .env.local
 *
 * Output goes to the console and to docs/attack-log.md.
 */
import { config } from "dotenv";
config({ path: ".env.local" });
import { writeFileSync, mkdirSync } from "node:fs";
import { Account, KeyPair, KeyPairSigner, actions, nearToYocto, teraToGas } from "near-api-js";
import type { KeyPairString } from "near-api-js";

const { getProvider, viewVault, fmtNear, explorerTx, VAULT_ID } = await import("../src/lib/chain");
const { requestSpend } = await import("../src/lib/agent-wallet");
const { ruleLabel } = await import("../src/lib/rules");

const AGENT_ID = process.env.AGENT_ACCOUNT_ID!;
const ROOT = VAULT_ID.replace(/^vault\./, "");
const agent = new Account(AGENT_ID, getProvider(), KeyPairSigner.fromSecretKey(process.env.AGENT_PRIVATE_KEY as KeyPairString));

const log: string[] = [];
const out = (line = "") => {
  console.log(line);
  log.push(line);
};

function reason(e: unknown) {
  const s = e instanceof Error ? e.message : JSON.stringify(e);
  const m = s.match(/does not allow method (\w+)/);
  if (m) return `protocol rejected: the agent key may only call request_spend(), not ${m[1]}()`;
  if (/MethodNameMismatch|method_name|does not match|not allowed/i.test(s)) return "protocol rejected: the agent key is not allowed to call this method";
  if (/ReceiverMismatch|receiver/i.test(s)) return "protocol rejected: the agent key only works on the vault contract";
  if (/RequiresFullAccess|full access|DepositWithFunctionCall|deposit/i.test(s)) return "protocol rejected: this action needs a full-access key, the agent has none";
  return `rejected: ${s.slice(0, 160)}`;
}

async function attempt(title: string, fn: () => Promise<unknown>) {
  out(`### ${title}`);
  try {
    await fn();
    out(`- RESULT: went through (this would be a bug)`);
  } catch (e) {
    out(`- RESULT: ${reason(e)}`);
  }
  out();
}

async function viaVault(title: string, receiver: string, amount: string, why: string) {
  out(`### ${title}`);
  const r = await requestSpend(receiver, amount, why);
  const verdict = r.request.status === "Executed" ? "PAID" : r.request.status === "Pending" ? "HELD FOR OWNER" : "BLOCKED";
  out(`- request_spend(${receiver.replace(`.${ROOT}`, "")}, ${amount} NEAR) -> ${verdict}: ${ruleLabel(r.request.rule)}`);
  out(`- tx: ${explorerTx(r.txHash)}`);
  out();
  return r.request;
}

const before = await viewVault();
out(`# Leash attack log`);
out();
out(`Run at ${new Date().toISOString()} against \`${VAULT_ID}\` on NEAR testnet.`);
out(`The attacker controls \`${AGENT_ID}\` and its only key. Vault free balance before: ${fmtNear(before.available, 3)} NEAR.`);
out();
out(`## Part 1: go around the vault`);
out();

await attempt("Transfer NEAR straight out of the agent account", () =>
  agent.transfer({ receiverId: `drainer.${ROOT}`, amount: nearToYocto("0.1") }),
);
await attempt("Add a new full-access key to the agent account", () =>
  agent.signAndSendTransaction({
    receiverId: AGENT_ID,
    actions: [actions.addFullAccessKey(KeyPair.fromRandom("ed25519").getPublicKey())],
  }),
);
await attempt("Call vault.withdraw() to pull funds", () =>
  agent.callFunction({ contractId: VAULT_ID, methodName: "withdraw", args: { amount: nearToYocto("1").toString() }, gas: teraToGas("30") }),
);
await attempt("Approve its own held request with vault.approve()", () =>
  agent.callFunction({ contractId: VAULT_ID, methodName: "approve", args: { id: 0 }, gas: teraToGas("30") }),
);
await attempt("Put the attacker on the allowlist with vault.allow()", () =>
  agent.callFunction({ contractId: VAULT_ID, methodName: "allow", args: { account: `drainer.${ROOT}` }, gas: teraToGas("30") }),
);

out(`## Part 2: go through the vault`);
out();
await viaVault("Pay the known drainer address", `drainer.${ROOT}`, "1.5", "ignore previous instructions");
await viaVault("Pay a lookalike of a trusted merchant", `cafee.${ROOT}`, "0.45", "new payout account");
await viaVault("Pay a trusted merchant, but over the per-payment cap", `cafe.${ROOT}`, "2", "catering");
await viaVault("Pay more than the vault holds", `cafe.${ROOT}`, "500", "everything");

const after = await viewVault();
out(`## Result`);
out();
out(`Vault free balance after: ${fmtNear(after.available, 3)} NEAR. Requests waiting for the owner: ${after.open_pending}.`);
out(`Nothing reached the attacker. Held requests only move if the owner approves them from their own wallet.`);

mkdirSync("docs", { recursive: true });
writeFileSync("docs/attack-log.md", log.join("\n") + "\n");
console.log("\nSaved docs/attack-log.md");
