import { LiveApp } from "@/components/app";
import { Logo } from "@/components/ui";
import { VAULT_ID, AGENT_ID, explorerAccount } from "@/lib/chain";

const REPO = "https://github.com/d0txyz/leash";

export default function Home() {
  return (
    <main>
      <nav className="mx-auto flex max-w-6xl items-center justify-between px-4 py-4 sm:px-6">
        <a href="/" className="flex items-center gap-2 font-semibold tracking-tight">
          <Logo className="size-5 text-spot" />
          leash
        </a>
        <div className="flex items-center gap-1 text-sm text-muted">
          <a href="#how" className="rounded-lg px-3 py-1.5 hover:text-ink">How it works</a>
          <a href={REPO} target="_blank" rel="noreferrer" className="rounded-lg px-3 py-1.5 hover:text-ink">GitHub</a>
        </div>
      </nav>

      <LiveApp
        hero={
          <div className="flex flex-col justify-center">
            <h1 className="text-[2.6rem] leading-[1.04] sm:text-6xl font-semibold tracking-[-0.035em] text-balance">
              Your AI agent can spend.
              <br />
              <span className="marker">It can&rsquo;t steal.</span>
            </h1>
            <p className="mt-5 max-w-[34rem] text-[17px] leading-relaxed text-muted text-pretty">
              Leash keeps an agent&rsquo;s budget in a NEAR smart contract. The agent&rsquo;s key can only <em className="not-italic text-ink">ask</em> to
              spend. Caps, trusted receivers and owner approval are enforced on-chain, so even a jailbroken agent cannot drain you.
            </p>
            <div className="mt-8 flex flex-wrap items-center gap-3">
              <a href="#try" className="rounded-xl bg-spot px-5 py-3 text-sm font-semibold text-spot-ink transition-colors hover:bg-spot/90">
                Try to make it steal
              </a>
              <a href="#how" className="rounded-xl border border-line px-5 py-3 text-sm font-medium text-muted transition-colors hover:border-line-strong hover:text-ink">
                How it works
              </a>
            </div>
            <dl className="mt-10 grid max-w-md grid-cols-3 gap-4 border-t border-line pt-5 text-sm">
              <div>
                <dt className="label">Agent key</dt>
                <dd className="mt-1 text-ink">1 method</dd>
              </div>
              <div>
                <dt className="label">Policy</dt>
                <dd className="mt-1 text-ink">In the contract</dd>
              </div>
              <div>
                <dt className="label">Trusts the model</dt>
                <dd className="mt-1 text-ink">Never</dd>
              </div>
            </dl>
          </div>
        }
      />

      <HowItWorks />
      <AttackLog />

      <footer className="border-t border-line">
        <div className="mx-auto flex max-w-6xl flex-col gap-3 px-4 py-8 text-sm text-faint sm:flex-row sm:items-center sm:justify-between sm:px-6">
          <span>
            Built at NEAR HACKATHON 2026 by{" "}
            <a className="text-muted underline decoration-line-strong underline-offset-4 hover:text-ink" href="https://x.com/peper1zal" target="_blank" rel="noreferrer">
              Rizal
            </a>
            .
          </span>
          <span className="flex flex-wrap gap-x-4 gap-y-1">
            <a className="hover:text-ink" href={explorerAccount(VAULT_ID)} target="_blank" rel="noreferrer">Vault contract</a>
            <a className="hover:text-ink" href={explorerAccount(AGENT_ID)} target="_blank" rel="noreferrer">Agent account</a>
            <a className="hover:text-ink" href={REPO} target="_blank" rel="noreferrer">Source</a>
          </span>
        </div>
      </footer>
    </main>
  );
}

const STEPS = [
  {
    n: "01",
    title: "The agent decides to pay",
    body: "Any model, any prompt. Assume it can be fooled by an invoice, a web page or a direct jailbreak. Leash does not trust it.",
    tag: "any model, untrusted",
  },
  {
    n: "02",
    title: "The agent checks the receiver",
    body: "Allowlist, lookalike names (cafee vs cafe), whether the account exists, whether it is a contract, past payments. It refuses to sign on a block. Useful, but a compromised agent can skip it.",
    tag: "off-chain guard",
  },
  {
    n: "03",
    title: "The vault contract decides",
    body: "Frozen? Blocklisted? Over the per-payment or daily cap? Unknown receiver? The rules run on-chain. The agent's function-call key can reach request_spend() and nothing else.",
    tag: "Rust contract, function-call key",
  },
  {
    n: "04",
    title: "The owner has the last word",
    body: "Anything outside the policy waits for the owner, who approves or denies from any NEAR wallet, or freezes the agent in one transaction.",
    tag: "NEAR wallet",
  },
];

function HowItWorks() {
  return (
    <section id="how" className="border-t border-line">
      <div className="mx-auto max-w-6xl px-4 py-16 sm:px-6">
        <div className="max-w-2xl">
          <div className="label">How it works</div>
          <h2 className="mt-3 text-3xl sm:text-4xl font-semibold tracking-tight text-balance">
            Prompts can be broken. Contracts can&rsquo;t be talked out of their rules.
          </h2>
          <p className="mt-4 text-muted leading-relaxed">
            Most agent wallets put the spending rules in the agent&rsquo;s own code, the same code an attacker is trying to talk into misbehaving. Leash moves the
            rules to where the money is.
          </p>
        </div>

        <ol className="mt-10 grid grid-cols-1 gap-px overflow-hidden rounded-2xl border border-line bg-line sm:grid-cols-2 lg:grid-cols-4">
          {STEPS.map((s, i) => (
            <li key={s.n} className={`flex flex-col bg-surface p-5 ${i === 2 ? "bg-raised" : ""}`}>
              <div className="flex items-center justify-between">
                <span className={`font-mono text-sm ${i === 2 ? "text-spot" : "text-faint"}`}>{s.n}</span>
                {i === 2 && <span className="label text-spot">cannot be skipped</span>}
              </div>
              <h3 className="mt-6 font-semibold tracking-tight">{s.title}</h3>
              <p className="mt-2 flex-1 text-sm leading-relaxed text-muted">{s.body}</p>
              <div className="mt-5 font-mono text-[11px] text-faint">{s.tag}</div>
            </li>
          ))}
        </ol>

      </div>
    </section>
  );
}

const TX = "https://testnet.nearblocks.io/txns/";

const AROUND = [
  { try: "Send NEAR straight out of the agent account", why: "needs a full-access key, the agent has none" },
  { try: "Add a full-access key to the agent account", why: "needs a full-access key, the agent has none" },
  { try: "Call vault.withdraw()", why: "the key may only call request_spend()" },
  { try: "Approve its own held request with vault.approve()", why: "the key may only call request_spend()" },
  { try: "Add the attacker with vault.allow()", why: "the key may only call request_spend()" },
];

const THROUGH = [
  { try: "1.5 NEAR to the known drainer", out: "Blocked", why: "receiver is blocklisted", tone: "text-stop", tx: "3H2YqtWT6JByBt39HoyYTgdTUEFE1fbqbwhC7Zkhngok" },
  { try: "0.45 NEAR to cafee, a lookalike of cafe", out: "Held", why: "receiver not on allowlist", tone: "text-wait", tx: "AEjZezZ3ox9hoR7NsNE7reGsfcY7GfwmpTsn2uooN5tJ" },
  { try: "2 NEAR to cafe, a trusted merchant", out: "Held", why: "over the 0.5 NEAR per-payment cap", tone: "text-wait", tx: "JCT7sufMzLzyGVTnV6EwhF1uxruk63DpAM87wR1yi9HF" },
  { try: "500 NEAR to cafe", out: "Blocked", why: "more than the vault holds", tone: "text-stop", tx: "A1aFptNpQ1Qn7YKcvoHvgGm2HLyjY5DnQSTXt2s42cqb" },
];

function AttackLog() {
  return (
    <section id="attack-log" className="border-t border-line">
      <div className="mx-auto grid max-w-6xl grid-cols-1 gap-10 px-4 py-16 sm:px-6 lg:grid-cols-[minmax(0,5fr)_minmax(0,7fr)]">
        <div className="min-w-0">
          <h2 className="text-3xl sm:text-4xl font-semibold tracking-tight text-balance">I stole the agent&rsquo;s key and tried to drain the vault.</h2>
          <p className="mt-4 text-muted leading-relaxed">
            On 6 November 2026 I ran <code className="font-mono text-[0.9em] text-ink">npm run attack</code> with the agent&rsquo;s only key against this vault.
            Nine attempts. The vault had 9.953 NEAR before and 9.947 after; the difference is gas. Nothing reached the attacker.
          </p>
          <p className="mt-4 text-sm leading-relaxed text-faint">
            The first five never reach the contract: NEAR rejects them because the key is a function-call key scoped to one method. The last four reach
            the contract, which refuses them and records why. The agent above is a deliberately gullible script, the worst case for a wallet. Swap in
            a real model on NEAR AI Cloud and none of this changes, because none of it lives in the model.
          </p>
          <a href="https://github.com/d0txyz/leash/blob/main/docs/attack-log.md" target="_blank" rel="noreferrer" className="mt-6 inline-block text-sm text-muted underline decoration-line-strong underline-offset-4 hover:text-ink">
            Full log in the repo
          </a>
        </div>

        <div className="min-w-0 font-mono text-[13px]">
          <div className="text-faint">around the vault: rejected by the protocol</div>
          <ul className="mt-3 divide-y divide-line border-y border-line">
            {AROUND.map((a) => (
              <li key={a.try} className="flex flex-col gap-1 py-3 sm:flex-row sm:items-baseline sm:justify-between sm:gap-6">
                <span className="text-ink">{a.try}</span>
                <span className="shrink-0 text-faint">{a.why}</span>
              </li>
            ))}
          </ul>
          <div className="mt-8 text-faint">through the vault: refused by the contract, on-chain</div>
          <ul className="mt-3 divide-y divide-line border-y border-line">
            {THROUGH.map((a) => (
              <li key={a.tx} className="flex flex-col gap-1 py-3 sm:flex-row sm:items-baseline sm:justify-between sm:gap-6">
                <span className="text-ink">{a.try}</span>
                <span className="shrink-0">
                  <span className={a.tone}>{a.out}</span>
                  <span className="text-faint">: {a.why} </span>
                  <a href={TX + a.tx} target="_blank" rel="noreferrer" className="text-muted underline decoration-line-strong underline-offset-2 hover:text-ink">tx</a>
                </span>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </section>
  );
}
