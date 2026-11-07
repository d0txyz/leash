// Server only. The hosted demo shares one vault between every visitor, so one
// visitor can use up the 3 NEAR daily allowance for everyone after them.
// The fix uses the same idea as the agent's wallet: a key on the owner account
// that can call exactly one vault method, reset_window(). It cannot move funds,
// approve, or change the policy.
//
// It only resets when the allowance is nearly gone AND nobody has used the
// vault for a few minutes, so a visitor working through the demo still sees
// the daily cap hold, and the next visitor starts fresh.
import { Account, KeyPairSigner, teraToGas } from "near-api-js";
import type { KeyPairString } from "near-api-js";
import { getProvider, viewRequests, viewVault, VAULT_ID, toYocto } from "./chain";

const LOW = toYocto("1");
const RECEIVER_LOW = toYocto("0.5");
const IDLE_MS = 3 * 60 * 1000;
let lastResetAt = 0;

export async function maybeResetWindow(): Promise<{ txHash: string } | null> {
  const id = process.env.DEMO_RESET_ACCOUNT_ID;
  const key = process.env.DEMO_RESET_PRIVATE_KEY as KeyPairString | undefined;
  if (!id || !key || Date.now() - lastResetAt < 60_000) return null;

  const vault = await viewVault();
  const left = BigInt(vault.daily_cap) - BigInt(vault.spent_today);
  // A receiver's own limit (e.g. the bookshop) can run out before the vault-wide cap.
  const receiverLow = (vault.receiver_caps ?? []).some((c) => BigInt(c.daily_cap) - BigInt(c.spent_today) < RECEIVER_LOW);
  if (left >= LOW && !receiverLow) return null;
  const [latest] = await viewRequests(1);
  if (latest && Date.now() - latest.created_at / 1e6 < IDLE_MS) return null;

  lastResetAt = Date.now();
  const owner = new Account(id, getProvider(), KeyPairSigner.fromSecretKey(key));
  const res = await owner.callFunctionRaw({ contractId: VAULT_ID, methodName: "reset_window", args: {}, gas: teraToGas("30") });
  return { txHash: res.transaction.hash };
}
