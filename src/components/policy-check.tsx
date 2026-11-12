"use client";

import { useRef, useState } from "react";
import { isAccountId, resolveAccount, shortAccount, toYocto, viewCheckSpend, type Decision } from "@/lib/chain";
import { toWei } from "@/lib/eth-links";
import { Pill, ruleLabel, type Tone } from "./ui";
import type { VaultData } from "./use-vault";

const EXAMPLES: [string, string][] = [
  ["cafe", "0.2"],
  ["cafe", "0.8"],
  ["cafee", "0.45"],
  ["drainer", "0.01"],
];
const ETH_EXAMPLES: [string, string][] = [
  ["apishop", "0.0005"],
  ["apishop", "0.01"],
  ["eth-drainer", "0.0001"],
];

const VERDICT: Record<string, { label: string; tone: Tone }> = {
  Executed: { label: "Would pay", tone: "paid" },
  Pending: { label: "Needs owner", tone: "wait" },
  Blocked: { label: "Blocked", tone: "stop" },
};

interface Result {
  decision: Decision;
  receiver: string;
  amount: string;
  unit: "NEAR" | "ETH";
}

/** Free dry run: calls the contract's check_spend view, so nothing is signed or spent. */
export function PolicyCheck({ eth }: { eth?: VaultData["eth"] }) {
  const [receiver, setReceiver] = useState("cafe");
  const [amount, setAmount] = useState("0.2");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<Result | null>(null);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);

  async function check(r = receiver, a = amount) {
    const name = r.trim().toLowerCase();
    const ethName = eth && name === "apishop" ? eth.shop : eth && name === "eth-drainer" ? eth.drainer : null;
    const id = ethName ?? resolveAccount(r);
    const onEth = /^0x[0-9a-f]{40}$/.test(id);
    if (!onEth && !isAccountId(id)) return setError(`"${r}" is not a NEAR account or 0x address`);
    const amountOk = onEth ? /^\d+(\.\d{1,18})?$/.test(a.trim()) && toWei(a) > 0n : /^\d+(\.\d{1,24})?$/.test(a.trim()) && toYocto(a) > 0n;
    if (!amountOk) return setError("Enter an amount above 0, like 0.2");
    const mine = ++seq.current;
    setBusy(true);
    setError(null);
    try {
      const decision = onEth ? await viewCheckSpend(id, toWei(a), "ethereum") : await viewCheckSpend(id, toYocto(a));
      if (mine === seq.current) setResult({ decision, receiver: ethName ? name : id, amount: a.trim(), unit: onEth ? "ETH" : "NEAR" });
    } catch (e) {
      if (mine === seq.current) setError(e instanceof Error && /check_spend/.test(e.message) ? "This vault predates check_spend" : "Could not reach NEAR testnet");
    } finally {
      if (mine === seq.current) setBusy(false);
    }
  }

  const v = result ? VERDICT[result.decision.status] ?? VERDICT.Blocked : null;

  return (
    <div className="mt-6 rounded-xl border border-line p-4">
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
        <div className="label">Ask the vault first</div>
        <span className="text-xs text-faint">
          free dry run, <code className="font-mono">check_spend()</code>, nothing is sent
        </span>
      </div>

      <form
        className="mt-3 flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          check();
        }}
      >
        <label className="sr-only" htmlFor="check-receiver">Receiver</label>
        <input
          id="check-receiver"
          value={receiver}
          onChange={(e) => setReceiver(e.target.value)}
          spellCheck={false}
          autoCapitalize="off"
          placeholder="receiver"
          className="h-9 min-w-0 flex-1 rounded-lg border border-line bg-surface px-3 font-mono text-sm placeholder:text-faint focus:border-line-strong focus:outline-none"
        />
        <label className="sr-only" htmlFor="check-amount">Amount (NEAR, or ETH for a 0x receiver)</label>
        <input
          id="check-amount"
          value={amount}
          onChange={(e) => setAmount(e.target.value)}
          inputMode="decimal"
          placeholder="amount"
          className="h-9 w-20 rounded-lg border border-line bg-surface px-3 text-right font-mono text-sm tabular placeholder:text-faint focus:border-line-strong focus:outline-none"
        />
        <button
          type="submit"
          disabled={busy}
          className="h-9 shrink-0 rounded-lg border border-line-strong px-3 text-sm font-medium transition-colors hover:bg-raised disabled:opacity-50"
        >
          {busy ? "Asking…" : "Check"}
        </button>
      </form>

      <div className="mt-2 flex flex-wrap gap-1.5">
        {[...EXAMPLES, ...(eth ? ETH_EXAMPLES : [])].map(([r, a]) => (
          <button
            key={r + a}
            type="button"
            onClick={() => {
              setReceiver(r);
              setAmount(a);
              check(r, a);
            }}
            className="rounded-md bg-raised/60 px-2 py-0.5 font-mono text-[11px] text-muted transition-colors hover:bg-raised hover:text-ink"
          >
            {r} {a}{ETH_EXAMPLES.some(([x]) => x === r) ? " ETH" : ""}
          </button>
        ))}
      </div>

      <div className="mt-3 min-h-[22px] text-sm" aria-live="polite">
        {error ? (
          <span className="text-stop">{error}</span>
        ) : result && v ? (
          <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <Pill tone={v.tone}>{v.label}</Pill>
            <span className="text-muted">
              <span className="font-mono text-ink">
                {result.amount} {result.unit}
              </span>{" "}
              to{" "}
              <span className="font-mono text-ink">{shortAccount(result.receiver)}</span>: {ruleLabel(result.decision.rule).toLowerCase()}
            </span>
          </span>
        ) : (
          <span className="text-faint">The same rules request_spend runs, as a free view call.</span>
        )}
      </div>
    </div>
  );
}
