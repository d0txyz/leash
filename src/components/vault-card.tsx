"use client";

import { useEffect, useState } from "react";
import { useNearWallet } from "near-connect-hooks";
import { VAULT_ID, AGENT_ID, explorerAccount, fmtNear, shortAccount, trippedForever, type AccessKeyInfo, type Vault } from "@/lib/chain";
import { ExternalLink, Pill } from "./ui";
import { PolicyCheck } from "./policy-check";

export function VaultCard({
  vault,
  agentKeys,
  onChanged,
}: {
  vault: Vault | null;
  agentKeys: AccessKeyInfo[];
  onChanged: () => void;
}) {
  const { signedAccountId, callFunction } = useNearWallet();
  const [busy, setBusy] = useState(false);
  const tripped = !!vault?.frozen && !vault.frozen_by_owner && !!vault.tripped_until;
  const now = useTicker(tripped);
  const isOwner = !!vault && signedAccountId === vault.owner;

  const spent = vault ? Number(BigInt(vault.spent_today) / 10n ** 20n) : 0;
  const cap = vault ? Number(BigInt(vault.daily_cap) / 10n ** 20n) : 1;
  const pct = Math.min(100, (spent / Math.max(cap, 1)) * 100);
  const fullKeys = agentKeys.filter((k) => k.permission === "FullAccess").length;
  const fcKey = agentKeys.find((k) => k.permission !== "FullAccess")?.permission;
  const fc = fcKey && typeof fcKey === "object" ? fcKey.FunctionCall : null;

  async function toggleFreeze() {
    if (!vault) return;
    setBusy(true);
    try {
      await callFunction({ contractId: VAULT_ID, method: vault.frozen ? "unfreeze" : "freeze", args: {}, gas: "30000000000000" });
      onChanged();
    } finally {
      setBusy(false);
    }
  }

  return (
    <section
      aria-label="Vault"
      className={`rounded-2xl border bg-surface/80 backdrop-blur p-5 sm:p-6 transition-colors ${vault?.frozen ? "border-stop/50" : "border-line"}`}
    >
      <div className="flex items-start justify-between gap-3">
        <div>
          <div className="label">Agent budget on NEAR testnet</div>
          <div className="mt-2 flex items-baseline gap-2">
            <span className="text-4xl sm:text-5xl font-semibold tracking-tight tabular">{vault ? fmtNear(vault.available, 2) : "…"}</span>
            <span className="text-muted text-lg">NEAR</span>
          </div>
          <div className="mt-1 text-sm text-faint">available to the agent, held by</div>
          <ExternalLink href={explorerAccount(VAULT_ID)} className="mt-0.5 break-all font-mono text-xs text-muted">
            {VAULT_ID}
          </ExternalLink>
        </div>
        {vault?.frozen && <Pill tone="stop">{tripped ? "Tripwire" : "Frozen"}</Pill>}
      </div>

      {tripped && vault?.tripped_until && (
        <div className="mt-4 rounded-lg bg-stop-soft px-3 py-2.5 text-sm leading-relaxed text-stop">
          The vault froze itself after too many refusals.{" "}
          {trippedForever(vault.tripped_until) ? (
            "Every request is refused until the owner unfreezes it."
          ) : (
            <>
              Every request is refused for <span className="font-mono tabular">{countdown(vault.tripped_until, now)}</span> more, or until the owner unfreezes it.
            </>
          )}
        </div>
      )}

      <div className="mt-6">
        <div className="flex justify-between text-sm">
          <span className="text-muted">Spent today without asking</span>
          <span className="tabular font-mono text-ink">
            {vault ? fmtNear(vault.spent_today, 2) : "0"} / {vault ? fmtNear(vault.daily_cap, 2) : "…"} NEAR
          </span>
        </div>
        <div className="mt-2 h-1.5 rounded-full bg-raised overflow-hidden" role="meter" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
          <div
            className={`h-full rounded-full transition-[width] duration-700 ${pct >= 100 ? "bg-wait" : "bg-paid"}`}
            style={{ width: `${pct}%` }}
          />
        </div>
      </div>

      <dl className="mt-6 grid grid-cols-1 gap-px overflow-hidden rounded-xl border border-line bg-line text-sm">
        <Rule term="Paid automatically" detail={`up to ${vault ? fmtNear(vault.per_tx_cap) : "…"} NEAR each, ${vault ? fmtNear(vault.daily_cap) : "…"} NEAR a day`} />
        <Rule term="Trusted receivers" detail={vault ? vault.allowlist.map(shortAccount).join(", ") || "none" : "…"} />
        {!!vault?.receiver_caps?.length && (
          <Rule
            term="Per-receiver limit"
            detail={vault.receiver_caps.map((c) => `${shortAccount(c.receiver)} ${fmtNear(c.daily_cap)} NEAR a day`).join(", ")}
          />
        )}
        <Rule term="Always refused" detail={vault ? vault.blocklist.map(([a]) => shortAccount(a)).join(", ") || "none" : "…"} tone="stop" />
        <Rule term="Everything else" detail="waits for the owner to approve" tone="wait" />
        {vault?.tripwire && (
          <Rule
            term="Tripwire"
            detail={`${vault.tripwire.max_blocked} refusals in ${minutes(vault.tripwire.window_secs)} freeze the vault${
              vault.tripwire.cooldown_secs ? ` for ${minutes(vault.tripwire.cooldown_secs)}` : ""
            } (${vault.recent_blocked ?? 0} so far)`}
            tone="stop"
          />
        )}
      </dl>

      <PolicyCheck />

      <div className="mt-6 rounded-xl border border-line bg-raised/60 p-4">
        <div className="flex items-center justify-between gap-3">
          <div className="label">The agent's only key</div>
          <ExternalLink href={explorerAccount(AGENT_ID)} className="text-xs text-faint">
            verify on explorer
          </ExternalLink>
        </div>
        <div className="mt-3 grid grid-cols-2 gap-3">
          <div>
            <div className="text-3xl font-semibold tabular">{agentKeys.length ? fullKeys : "…"}</div>
            <div className="text-xs text-muted mt-0.5">full-access keys</div>
          </div>
          <div className="min-w-0">
            <div className="font-mono text-sm text-paid truncate">{fc ? fc.method_names.join(", ") : "…"}()</div>
            <div className="text-xs text-muted mt-1">the one method it can call</div>
          </div>
        </div>
        <p className="mt-3 text-xs leading-relaxed text-faint">
          It cannot transfer NEAR, add keys, approve, or touch the policy. A stolen or jailbroken agent can only ask.
        </p>
      </div>

      <div className="mt-5 flex flex-wrap items-center justify-between gap-3 text-xs text-faint">
        <span>
          Owner <span className="font-mono text-muted">{vault?.owner ?? "…"}</span>
        </span>
        {isOwner && (
          <button
            onClick={toggleFreeze}
            disabled={busy}
            className={`rounded-lg px-3 py-1.5 text-sm font-medium transition-colors disabled:opacity-50 ${
              vault?.frozen ? "bg-paid text-spot-ink hover:bg-paid/90" : "bg-stop text-spot-ink hover:bg-stop/90"
            }`}
          >
            {busy ? "Confirm in wallet…" : vault?.frozen ? "Unfreeze agent" : "Freeze agent"}
          </button>
        )}
      </div>
    </section>
  );
}

/** Re-renders once a second while `on`, for the tripwire countdown. */
function useTicker(on: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [on]);
  return now;
}

function countdown(untilNs: number, now: number) {
  const s = Math.max(0, Math.ceil(untilNs / 1e6 / 1000 - now / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

function minutes(secs: number) {
  const m = Math.round(secs / 60);
  return m >= 1 ? `${m} min` : `${secs} s`;
}

function Rule({ term, detail, tone }: { term: string; detail: string; tone?: "stop" | "wait" }) {
  return (
    <div className="flex items-baseline justify-between gap-4 bg-surface px-4 py-3">
      <dt className="text-muted shrink-0">{term}</dt>
      <dd className={`min-w-0 text-right font-mono text-[13px] text-balance break-words ${tone === "stop" ? "text-stop" : tone === "wait" ? "text-wait" : "text-ink"}`}>{detail}</dd>
    </div>
  );
}
