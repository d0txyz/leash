"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { AccessKeyInfo, SpendRequest, Vault } from "@/lib/chain";

export interface VaultData {
  vault: Vault;
  requests: SpendRequest[];
  agentKeys: AccessKeyInfo[];
  /** The vault's Chain Signatures address on Sepolia, when cross-chain is on. */
  eth: { address: string; balance: string | null; shop: string; drainer: string } | null;
  at: number;
}

export function useVault(intervalMs = 3000) {
  const [data, setData] = useState<VaultData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const busy = useRef(false);

  const refresh = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    try {
      const res = await fetch("/api/vault", { cache: "no-store" });
      const json = await res.json();
      if (!res.ok) throw new Error(json.error ?? "Could not read the vault");
      setData(json);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not read the vault");
    } finally {
      busy.current = false;
    }
  }, []);

  useEffect(() => {
    refresh();
    const id = setInterval(() => {
      if (document.visibilityState === "visible") refresh();
    }, intervalMs);
    return () => clearInterval(id);
  }, [refresh, intervalMs]);

  return { data, error, refresh };
}
