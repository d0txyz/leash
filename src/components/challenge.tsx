"use client";

import { VAULT_ID, explorerAccount, fmtNear, shortAccount, type SpendRequest, type Vault } from "@/lib/chain";
import { fmtEth } from "@/lib/eth-links";
import { StatusPill, ruleLabel, timeAgo } from "./ui";

const ROOT = VAULT_ID.replace(/^vault\./, "");

/**
 * The public dare. Every number is read from the contract's own counters
 * (get_vault().stats), so nothing here is tallied by this website.
 */
export function Challenge({ vault, requests }: { vault: Vault | null; requests: SpendRequest[] | null }) {
  const s = vault?.stats;
  const refused = s ? s.blocked + s.denied : null;
  const kept = s ? BigInt(s.near_blocked) + BigInt(s.near_denied) : null;
  const latest = (requests ?? []).filter((r) => r.status === "Blocked" || r.status === "Denied").slice(0, 6);

  return (
    <section id="challenge" className="border-t border-line">
      <div className="mx-auto grid max-w-6xl grid-cols-1 gap-10 px-4 py-16 sm:px-6 lg:grid-cols-[minmax(0,5fr)_minmax(0,7fr)]">
        <div className="min-w-0">
          <h2 className="text-3xl sm:text-4xl font-semibold tracking-tight text-balance">Try to steal it.</h2>
          <p className="mt-4 text-muted leading-relaxed">
            Get one payment to <span className="font-mono text-[0.9em] text-ink">drainer.{ROOT}</span>. Flip the jailbreak, paste a poisoned invoice,
            write your own attack. Every refusal is recorded on-chain and counted by the contract itself.
          </p>
          <p className="mt-4 text-sm leading-relaxed text-faint">
            Keep at it and the tripwire answers: {vault?.tripwire ? `${vault.tripwire.max_blocked} refusals in ${Math.round(vault.tripwire.window_secs / 60)} minutes` : "repeated refusals"}{" "}
            and the vault freezes itself, even for the agent&rsquo;s normal purchases.
          </p>
          <a
            href="#try"
            className="mt-6 inline-flex h-10 items-center rounded-md bg-spot px-4 text-sm font-medium text-spot-ink transition-colors hover:bg-spot/90"
          >
            Open the agent
          </a>
        </div>

        <div className="min-w-0">
          <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-xl border border-line bg-line sm:grid-cols-4">
            <Stat label="requests refused" value={refused === null ? "…" : refused.toLocaleString("en-US")} />
            <Stat label="NEAR refused" value={kept === null ? "…" : fmtNear(kept, 2)} />
            <Stat label="tripwire freezes" value={s ? s.tripwire_trips.toLocaleString("en-US") : "…"} />
            <Stat label="payments made" value={s ? s.paid.toLocaleString("en-US") : "…"} />
          </dl>
          <p className="mt-3 text-xs leading-relaxed text-faint">
            Every one of the {s ? s.paid.toLocaleString("en-US") : "…"} payments was inside the policy or approved by the owner. Counted by{" "}
            <a href={explorerAccount(VAULT_ID)} target="_blank" rel="noreferrer" className="underline decoration-line-strong underline-offset-2 hover:text-ink">
              the vault contract
            </a>{" "}
            in <code className="font-mono">get_vault().stats</code>.
          </p>

          <div className="mt-8 font-mono text-[13px]">
            <div className="text-faint">latest refusals</div>
            {latest.length === 0 ? (
              <p className="mt-3 border-y border-line py-3 text-faint">{requests ? "None in the latest requests. Be the first." : "Reading the vault…"}</p>
            ) : (
              <ul className="mt-3 divide-y divide-line border-y border-line">
                {latest.map((r) => (
                  <li key={r.id} className="py-3">
                    <div className="flex items-baseline justify-between gap-4">
                      <span className="min-w-0 text-ink">
                        {r.chain === "ethereum" ? `${fmtEth(r.amount)} ETH on Sepolia` : `${fmtNear(r.amount, 3)} NEAR`} to {shortAccount(r.receiver)}
                        <span className="text-faint"> · {timeAgo(r.created_at)}</span>
                      </span>
                      <StatusPill status={r.status} />
                    </div>
                    <div className="mt-1 text-faint">{ruleLabel(r.rule).toLowerCase()}</div>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      </div>
    </section>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col-reverse justify-end bg-surface px-4 py-4">
      <dt className="mt-1 text-xs text-muted">{label}</dt>
      <dd className="text-3xl font-semibold tracking-tight tabular">{value}</dd>
    </div>
  );
}
