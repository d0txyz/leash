/**
 * One-time testnet setup. Creates every account Leash needs, deploys the vault,
 * and writes the keys to .env.local (gitignored). Safe to re-run: it reuses
 * the root account if .env.local already has one.
 *
 *   npm run contract:build && npm run setup:testnet
 *
 * Accounts (all sub-accounts of one faucet-funded root):
 *   vault.<root>    the Leash contract, holds the agent's budget
 *   agent.<root>    the AI agent. It has NO full-access key: its only key is a
 *                   function-call key that can call vault.request_spend, nothing else
 *   cafe.<root>     demo merchant, on the allowlist
 *   books.<root>    demo merchant, on the allowlist
 *   drainer.<root>  demo attacker, on the blocklist
 *   cafee.<root>    demo lookalike of cafe, not on any list (the guard catches it)
 * The root account is the vault owner (approves, denies, freezes).
 */
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { Account, JsonRpcProvider, KeyPair, KeyPairSigner, actions, nearToYocto, teraToGas } from "near-api-js";
import type { KeyPairString } from "near-api-js";

const RPC = process.env.NEAR_RPC_URL ?? "https://test.rpc.fastnear.com";
const FAUCET = "https://helper.nearprotocol.com/account";
const WASM = "contract/target/near/leash_vault.wasm";
const ENV_FILE = ".env.local";

const POLICY = { perTxCap: "0.5", dailyCap: "3", booksDailyCap: "1.2" } as const; // NEAR
const VAULT_FUNDING = "7" as const;
const AGENT_FUNDING = "1"; // pays the agent's own gas
const AGENT_KEY_ALLOWANCE = "0.5"; // max gas the agent key may ever burn

const provider = new JsonRpcProvider({ url: RPC });

function readEnv(): Record<string, string> {
  if (!existsSync(ENV_FILE)) return {};
  return Object.fromEntries(
    readFileSync(ENV_FILE, "utf8")
      .split(/\r?\n/)
      .filter((l) => l.includes("=") && !l.startsWith("#"))
      .map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]),
  );
}

function saveEnv(values: Record<string, string>) {
  const merged = { ...readEnv(), ...values };
  writeFileSync(
    ENV_FILE,
    "# Written by scripts/setup-testnet.ts. Never commit this file.\n" +
      Object.entries(merged).map(([k, v]) => `${k}=${v}`).join("\n") +
      "\n",
  );
}

async function exists(accountId: string) {
  try {
    await provider.viewAccount({ accountId });
    return true;
  } catch {
    return false;
  }
}

async function waitFor(accountId: string) {
  for (let i = 0; i < 30; i++) {
    if (await exists(accountId)) return;
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error(`${accountId} did not appear on testnet`);
}

async function main() {
  if (!existsSync(WASM)) throw new Error(`Missing ${WASM}. Run: npm run contract:build`);
  const env = readEnv();

  // 1. Root account from the testnet faucet
  let rootId = env.ROOT_ACCOUNT_ID;
  let rootKey = env.ROOT_PRIVATE_KEY as KeyPairString | undefined;
  if (!rootId || !rootKey) {
    const kp = KeyPair.fromRandom("ed25519");
    rootId = `leash-${Math.random().toString(36).slice(2, 8)}.testnet`;
    rootKey = kp.toString() as KeyPairString;
    console.log(`Creating ${rootId} with the testnet faucet...`);
    const res = await fetch(FAUCET, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ newAccountId: rootId, newAccountPublicKey: kp.getPublicKey().toString() }),
    });
    if (!res.ok) throw new Error(`Faucet failed: ${res.status} ${await res.text()}`);
    saveEnv({ ROOT_ACCOUNT_ID: rootId, ROOT_PRIVATE_KEY: rootKey });
    await waitFor(rootId);
  } else {
    console.log(`Reusing ${rootId}`);
  }
  const root = new Account(rootId, provider, KeyPairSigner.fromSecretKey(rootKey));

  const vaultId = `vault.${rootId}`;
  const agentId = `agent.${rootId}`;
  const merchants = [`cafe.${rootId}`, `books.${rootId}`];
  const attacker = `drainer.${rootId}`;
  const lookalike = `cafee.${rootId}`;
  const rootPublicKey = KeyPair.fromString(rootKey).getPublicKey();

  // 2. Demo merchants and attacker (controlled by root, so no funds really leave)
  for (const id of [...merchants, attacker, lookalike]) {
    if (await exists(id)) continue;
    console.log(`Creating ${id}`);
    await root.createSubAccount({ accountOrPrefix: id.split(".")[0], publicKey: rootPublicKey, nearToTransfer: nearToYocto("0.1") });
  }

  // 3. Agent account with ONLY a function-call key
  let agentKey = env.AGENT_PRIVATE_KEY as KeyPairString | undefined;
  if (!(await exists(agentId))) {
    const kp = KeyPair.fromRandom("ed25519");
    agentKey = kp.toString() as KeyPairString;
    saveEnv({ AGENT_ACCOUNT_ID: agentId, AGENT_PRIVATE_KEY: agentKey });
    console.log(`Creating ${agentId} (function-call key only, no full-access key)`);
    await root.signAndSendTransaction({
      receiverId: agentId,
      actions: [
        actions.createAccount(),
        actions.transfer(nearToYocto(AGENT_FUNDING)),
        actions.addFunctionCallAccessKey(kp.getPublicKey(), vaultId, ["request_spend"], nearToYocto(AGENT_KEY_ALLOWANCE)),
      ],
    });
  } else if (!agentKey) {
    throw new Error(`${agentId} exists but AGENT_PRIVATE_KEY is missing from ${ENV_FILE}`);
  }

  // 4. Vault: create, deploy, init, set policy
  if (!(await exists(vaultId))) {
    console.log(`Creating and deploying ${vaultId}`);
    const wasm = new Uint8Array(readFileSync(WASM));
    await root.signAndSendTransaction({
      receiverId: vaultId,
      actions: [
        actions.createAccount(),
        actions.transfer(nearToYocto(VAULT_FUNDING)),
        actions.addFullAccessKey(rootPublicKey),
        actions.deployContract(wasm),
        actions.functionCall(
          "new",
          {
            owner: rootId,
            agent: agentId,
            per_tx_cap: nearToYocto(POLICY.perTxCap).toString(),
            daily_cap: nearToYocto(POLICY.dailyCap).toString(),
          },
          teraToGas("30"),
          0n,
        ),
      ],
    });
    for (const m of merchants) {
      await root.callFunction({ contractId: vaultId, methodName: "allow", args: { account: m }, gas: teraToGas("30") });
    }
    await root.callFunction({
      contractId: vaultId,
      methodName: "block",
      args: { account: attacker, reason: "reported drainer address" },
      gas: teraToGas("30"),
    });
    // Trusted is not unlimited: the bookshop gets at most this much a day.
    await root.callFunction({
      contractId: vaultId,
      methodName: "set_receiver_cap",
      args: { receiver: merchants[1], daily_cap: nearToYocto(POLICY.booksDailyCap).toString() },
      gas: teraToGas("30"),
    });
    // 3 refusals in 10 minutes freeze the vault for 2 minutes. Drop cooldown_secs
    // to keep it frozen until the owner unfreezes it.
    await root.callFunction({
      contractId: vaultId,
      methodName: "set_tripwire",
      args: { max_blocked: 3, window_secs: 600, cooldown_secs: 120 },
      gas: teraToGas("30"),
    });
  } else {
    console.log(`${vaultId} already exists, skipping deploy`);
  }

  const out = {
    NEAR_NETWORK: "testnet",
    NEAR_RPC_URL: RPC,
    NEXT_PUBLIC_NEAR_NETWORK: "testnet",
    NEXT_PUBLIC_NEAR_RPC_URL: RPC,
    NEXT_PUBLIC_VAULT_ID: vaultId,
    NEXT_PUBLIC_AGENT_ID: agentId,
    DEMO_MERCHANTS: merchants.join(","),
    DEMO_ATTACKER: attacker,
    ROOT_ACCOUNT_ID: rootId,
    ROOT_PRIVATE_KEY: rootKey,
    AGENT_ACCOUNT_ID: agentId,
    AGENT_PRIVATE_KEY: agentKey!,
    NEAR_AI_API_KEY: env.NEAR_AI_API_KEY ?? "",
    NEAR_AI_MODEL: env.NEAR_AI_MODEL ?? "",
  };
  writeFileSync(
    ENV_FILE,
    "# Written by scripts/setup-testnet.ts. Never commit this file.\n" +
      Object.entries(out).map(([k, v]) => `${k}=${v}`).join("\n") +
      "\n",
  );

  const vault = await provider.callFunction({ contractId: vaultId, method: "get_vault", args: {} });
  const keys = await provider.viewAccessKeyList({ accountId: agentId });
  console.log("\nDone. Vault state:");
  console.log(JSON.stringify(vault, null, 2));
  console.log(`\nAgent keys on ${agentId}:`);
  for (const k of keys.keys) console.log(` - ${JSON.stringify(k.access_key.permission)}`);
  console.log(`\nWrote ${ENV_FILE} (keys are in there, do not commit it).`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
