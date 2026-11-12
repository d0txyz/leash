"use client";

import { useVault } from "./use-vault";
import { VaultCard } from "./vault-card";
import { Ledger } from "./ledger";
import { AgentConsole } from "./agent-console";
import { Challenge } from "./challenge";

/** The live part of the page: one poller feeds the vault card and the ledger. */
export function LiveApp({ hero }: { hero: React.ReactNode }) {
  const { data, error, refresh } = useVault();
  return (
    <>
      <section className="mx-auto grid max-w-6xl grid-cols-1 gap-10 px-4 pt-10 pb-12 sm:px-6 lg:grid-cols-[1.05fr_1fr] lg:gap-14 lg:pt-16">
        {hero}
        <div>
          <VaultCard vault={data?.vault ?? null} agentKeys={data?.agentKeys ?? []} eth={data?.eth ?? null} onChanged={refresh} />
          {error && <p className="mt-2 text-xs text-stop">Could not reach NEAR testnet: {error}</p>}
        </div>
      </section>

      <section className="mx-auto max-w-6xl px-4 pb-16 sm:px-6">
        <div className="grid grid-cols-1 gap-5 lg:h-[720px] lg:grid-cols-[1.25fr_1fr]">
          <AgentConsole onActivity={refresh} />
          <div className="flex h-[560px] min-h-0 min-w-0 lg:h-auto">
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">
              <Ledger requests={data?.requests ?? null} vault={data?.vault ?? null} eth={data?.eth ?? null} onChanged={refresh} />
            </div>
          </div>
        </div>
      </section>

      <Challenge vault={data?.vault ?? null} requests={data?.requests ?? null} />
    </>
  );
}
