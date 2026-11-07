// Server only. The agent's wallet: a function-call key that can call exactly one
// method, `request_spend` on the vault. It cannot transfer NEAR, add keys, or
// call any owner method, even if the agent is fully compromised.
import { Account, KeyPairSigner, teraToGas } from "near-api-js";
import type { KeyPairString } from "near-api-js";
import { getProvider, VAULT_ID, toYocto, type SpendRequest } from "./chain";

export interface SpendResult {
  request: SpendRequest;
  txHash: string;
}

let account: Account | null = null;
function agentAccount() {
  const id = process.env.AGENT_ACCOUNT_ID;
  const key = process.env.AGENT_PRIVATE_KEY as KeyPairString | undefined;
  if (!id || !key) throw new Error("AGENT_ACCOUNT_ID / AGENT_PRIVATE_KEY are not set. Run npm run setup:testnet.");
  account ??= new Account(id, getProvider(), KeyPairSigner.fromSecretKey(key));
  return account;
}

export async function requestSpend(receiver: string, amountNear: string, reason: string): Promise<SpendResult> {
  const res = await agentAccount().callFunctionRaw({
    contractId: VAULT_ID,
    methodName: "request_spend",
    args: { receiver, amount: toYocto(amountNear).toString(), reason },
    gas: teraToGas("50"),
  });
  const status = res.status as { SuccessValue?: string; Failure?: unknown };
  if (status.SuccessValue === undefined) {
    throw new Error(`request_spend failed: ${JSON.stringify(status.Failure ?? status)}`);
  }
  const request = JSON.parse(Buffer.from(status.SuccessValue, "base64").toString("utf8")) as SpendRequest;
  return { request, txHash: res.transaction.hash };
}
