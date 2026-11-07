// Counterparty check the agent runs before it signs anything.
// This is the agent-side layer: helpful, but bypassable if the agent is compromised.
// The vault contract is the layer that cannot be bypassed.
import { getProvider, viewRequests, type Vault } from "./chain";

export type Verdict = "allow" | "warn" | "block";
export interface Signal {
  level: Verdict;
  label: string;
}
export interface CheckResult {
  receiver: string;
  verdict: Verdict;
  signals: Signal[];
}

const SUSPICIOUS = ["drain", "refund", "airdrop", "claim", "support", "recover", "giveaway", "bonus", "verify"];

function firstLabel(id: string) {
  return id.split(".")[0];
}

function normalize(s: string) {
  return s.replace(/0/g, "o").replace(/[1|]/g, "l").replace(/5/g, "s").replace(/rn/g, "m").replace(/[-_]/g, "");
}

function distance(a: string, b: string) {
  const dp = Array.from({ length: a.length + 1 }, (_, i) => [i, ...Array(b.length).fill(0)]);
  for (let j = 1; j <= b.length; j++) dp[0][j] = j;
  for (let i = 1; i <= a.length; i++)
    for (let j = 1; j <= b.length; j++)
      dp[i][j] = Math.min(dp[i - 1][j] + 1, dp[i][j - 1] + 1, dp[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
  return dp[a.length][b.length];
}

/** Returns the allowlisted account this one imitates, if any. */
export function lookalikeOf(receiver: string, allowlist: string[]): string | null {
  if (allowlist.includes(receiver)) return null;
  const r = firstLabel(receiver);
  for (const a of allowlist) {
    const l = firstLabel(a);
    if (r === l) return a; // same name, different parent: cafe.evil.testnet vs cafe.leash-x.testnet
    if (normalize(r) === normalize(l)) return a;
    if (Math.min(r.length, l.length) >= 4 && distance(r, l) <= 2) return a;
  }
  return null;
}

export async function checkCounterparty(receiver: string, vault: Vault): Promise<CheckResult> {
  const signals: Signal[] = [];
  const blocked = vault.blocklist.find(([a]) => a === receiver);
  if (blocked) signals.push({ level: "block", label: `On the vault blocklist: ${blocked[1]}` });

  if (vault.allowlist.includes(receiver)) {
    signals.push({ level: "allow", label: "Known merchant on the owner's allowlist" });
  } else {
    const twin = lookalikeOf(receiver, vault.allowlist);
    if (twin) signals.push({ level: "block", label: `Lookalike of trusted merchant ${twin}` });
    else signals.push({ level: "warn", label: "Not on the owner's allowlist" });
  }

  try {
    const acct = await getProvider().viewAccount({ accountId: receiver });
    const code = String((acct as { code_hash?: string }).code_hash ?? "");
    if (code && code !== "11111111111111111111111111111111") {
      signals.push({ level: "warn", label: "Receiver is a smart contract" });
    }
  } catch {
    signals.push({ level: "block", label: "Account does not exist on NEAR" });
  }

  if (/^[0-9a-f]{64}$/.test(receiver)) signals.push({ level: "warn", label: "Anonymous implicit account" });
  const word = SUSPICIOUS.find((w) => receiver.toLowerCase().includes(w));
  if (word) signals.push({ level: "warn", label: `Name contains "${word}"` });

  try {
    const history = await viewRequests(100);
    const paid = history.filter((r) => r.receiver === receiver && (r.status === "Executed" || r.status === "Approved")).length;
    if (paid > 0) signals.push({ level: "allow", label: `Paid ${paid} time${paid > 1 ? "s" : ""} before` });
  } catch {
    // history is a nice-to-have signal
  }

  const verdict: Verdict = signals.some((s) => s.level === "block")
    ? "block"
    : signals.some((s) => s.level === "warn")
      ? "warn"
      : "allow";
  return { receiver, verdict, signals };
}
