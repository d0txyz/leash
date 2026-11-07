"use client";

import { useEffect, useRef, useState } from "react";
import { useNearWallet } from "near-connect-hooks";
import { VAULT_ID, explorerAccount, fmtNear, shortAccount, type SpendRequest, type Vault } from "@/lib/chain";
import { StatusPill, ruleLabel, timeAgo } from "./ui";

export function Ledger({
  requests,
  vault,
  onChanged,
}: {
  requests: SpendRequest[] | null;
  vault: Vault | null;
  onChanged: () => void;
}) {
  const { signedAccountId, signIn, signOut, callFunction } = useNearWallet();
  const isOwner = !!vault && signedAccountId === vault.owner;
  const seen = useRef<Set<number> | null>(null);
  const [fresh, setFresh] = useState<Set<number>>(new Set());
  const [busyId, setBusyId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!requests) return;
    if (seen.current === null) {
      seen.current = new Set(requests.map((r) => r.id));
      return;
    }
    const added = requests.filter((r) => !seen.current!.has(r.id)).map((r) => r.id);
    added.forEach((id) => seen.current!.add(id));
    if (added.length) setFresh(new Set(added));
  }, [requests]);

  async function act(id: number, method: "approve" | "deny") {
    setBusyId(id);
    setError(null);
    try {
      await callFunction({ contractId: VAULT_ID, method, args: { id }, gas: "60000000000000" });
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : "The wallet did not send the transaction.");
    } finally {
      setBusyId(null);
    }
  }

  const pending = requests?.filter((r) => r.status === "Pending").length ?? 0;

  return (
    <section aria-label="On-chain ledger" className="flex min-h-0 flex-1 flex-col rounded-2xl border border-line bg-surface/80">
      <header className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-5 py-4">
        <div>
          <h2 className="font-semibold tracking-tight">Vault ledger</h2>
          <p className="text-xs text-faint mt-0.5">
            Every request the agent made, read from the{" "}
            <a href={explorerAccount(VAULT_ID)} target="_blank" rel="noreferrer" className="underline decoration-line-strong underline-offset-2 hover:text-ink">
              contract
            </a>
            . Refusals are recorded too.
          </p>
        </div>
        <div className="flex items-center gap-2 text-xs">
          {pending > 0 && <span className="text-wait tabular">{pending} waiting</span>}
          {signedAccountId ? (
            <button onClick={() => signOut().catch(() => {})} className="rounded-lg border border-line px-2.5 py-1.5 text-muted hover:text-ink hover:border-line-strong" title="Sign out">
              {isOwner ? "Owner" : "Viewer"} · <span className="font-mono">{signedAccountId.length > 22 ? signedAccountId.slice(0, 20) + "…" : signedAccountId}</span>
            </button>
          ) : (
            <button onClick={() => signIn().catch(() => {})} className="rounded-lg border border-line px-2.5 py-1.5 text-muted hover:text-ink hover:border-line-strong">
              Owner sign in
            </button>
          )}
        </div>
      </header>

      {error && <div className="mx-5 mt-3 rounded-lg bg-stop-soft px-3 py-2 text-xs text-stop">{error}</div>}

      <ol className="scroll-thin min-h-0 flex-1 overflow-y-auto divide-y divide-line">
        {!requests &&
          Array.from({ length: 6 }).map((_, i) => (
            <li key={i} className="px-5 py-4">
              <div className="h-4 w-2/3 rounded bg-raised animate-pulse" />
              <div className="mt-2 h-3 w-1/3 rounded bg-raised animate-pulse" />
            </li>
          ))}
        {requests?.length === 0 && <li className="px-5 py-10 text-center text-sm text-faint">No requests yet. Ask the agent to buy something.</li>}
        {requests?.map((r) => (
          <li key={r.id} className={`px-5 py-3.5 ${fresh.has(r.id) ? "animate-row-in" : ""}`}>
            <div className="flex items-center justify-between gap-3">
              <div className="flex min-w-0 items-center gap-2.5">
                <StatusPill status={r.status} />
                <span className="min-w-0 truncate text-sm">
                  <span className="tabular font-medium">{fmtNear(r.amount, 3)} NEAR</span>
                  <span className="text-faint"> to </span>
                  <span className="font-mono text-[13px] text-muted">{shortAccount(r.receiver)}</span>
                </span>
              </div>
              <span className="shrink-0 text-xs text-faint tabular">#{r.id} · {timeAgo(r.created_at)}</span>
            </div>
            <div className="mt-1.5 flex items-start justify-between gap-3">
              <p className="min-w-0 text-xs text-faint">
                <span className={r.status === "Blocked" ? "text-stop/90" : r.status === "Pending" ? "text-wait/90" : "text-muted"}>{ruleLabel(r.rule)}</span>
                {r.reason && <span className="text-faint"> · &ldquo;{r.reason.length > 80 ? r.reason.slice(0, 80) + "…" : r.reason}&rdquo;</span>}
              </p>
              {r.status === "Pending" &&
                (isOwner ? (
                  <div className="flex shrink-0 gap-1.5">
                    <button
                      onClick={() => act(r.id, "deny")}
                      disabled={busyId !== null}
                      className="rounded-md border border-line px-2.5 py-1 text-xs text-muted hover:text-ink hover:border-line-strong disabled:opacity-50"
                    >
                      Deny
                    </button>
                    <button
                      onClick={() => act(r.id, "approve")}
                      disabled={busyId !== null}
                      className="rounded-md bg-ink px-2.5 py-1 text-xs font-medium text-bg hover:bg-ink/85 disabled:opacity-50"
                    >
                      {busyId === r.id ? "Signing…" : "Approve"}
                    </button>
                  </div>
                ) : (
                  <span className="shrink-0 text-[11px] text-faint">owner decides</span>
                ))}
            </div>
          </li>
        ))}
      </ol>
    </section>
  );
}
