"use client";

import type { Status } from "@/lib/chain";

export const STATUS_META: Record<Status, { label: string; tone: Tone }> = {
  Executed: { label: "Paid", tone: "paid" },
  Approved: { label: "Approved", tone: "paid" },
  Pending: { label: "Needs owner", tone: "wait" },
  Denied: { label: "Denied", tone: "idle" },
  Blocked: { label: "Blocked", tone: "stop" },
  Failed: { label: "Bounced", tone: "idle" },
  Signing: { label: "Signing", tone: "wait" },
};

export type Tone = "paid" | "wait" | "stop" | "idle";

const TONE_CLASS: Record<Tone, string> = {
  paid: "text-paid bg-paid-soft",
  wait: "text-wait bg-wait-soft",
  stop: "text-stop bg-stop-soft",
  idle: "text-idle bg-idle-soft",
};

export function Pill({ tone, children, className = "" }: { tone: Tone; children: React.ReactNode; className?: string }) {
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-[11px] font-medium tracking-wide whitespace-nowrap ${TONE_CLASS[tone]} ${className}`}
    >
      <span className="size-1.5 rounded-full bg-current" />
      {children}
    </span>
  );
}

export function StatusPill({ status }: { status: Status }) {
  const m = STATUS_META[status];
  return <Pill tone={m.tone}>{m.label}</Pill>;
}

export { ruleLabel } from "@/lib/rules";

export function timeAgo(ns: number) {
  const s = Math.max(0, Math.round((Date.now() - ns / 1e6) / 1000));
  if (s < 60) return `${s}s ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.round(h / 24)}d ago`;
}

export function ExternalLink({ href, children, className = "" }: { href: string; children: React.ReactNode; className?: string }) {
  return (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      className={`inline-flex items-center gap-1 underline decoration-line-strong underline-offset-4 hover:decoration-ink hover:text-ink transition-colors ${className}`}
    >
      {children}
      <svg aria-hidden viewBox="0 0 12 12" className="size-2.5 opacity-70">
        <path d="M4 2h6v6M10 2 3 9" fill="none" stroke="currentColor" strokeWidth="1.4" />
      </svg>
    </a>
  );
}

export function Logo({ className = "" }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden className={className}>
      <circle cx="6" cy="17" r="3.2" fill="none" stroke="currentColor" strokeWidth="2" />
      <path d="M8.4 14.8C12 11 13 8 18.5 5.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
      <circle cx="19" cy="5.2" r="1.9" fill="currentColor" />
    </svg>
  );
}
