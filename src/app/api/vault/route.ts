import { NextResponse } from "next/server";
import { viewVault, viewRequests, viewAgentKeys, type AccessKeyInfo, type SpendRequest } from "@/lib/chain";
import { demoEthAccounts, deriveEthAddress, ethBalance, signedRawTx } from "@/lib/eth";

export const dynamic = "force-dynamic";

let keysCache: { at: number; keys: AccessKeyInfo[] } | null = null;

export interface EthInfo {
  address: string;
  balance: string | null;
  shop: string;
  drainer: string;
}
let ethCache: { at: number; info: EthInfo } | null = null;

/** The vault's Chain Signatures address and its Sepolia balance. Best effort. */
async function ethInfo(): Promise<EthInfo | null> {
  if (ethCache && Date.now() - ethCache.at < 15_000) return ethCache.info;
  try {
    const [address, demo] = await Promise.all([deriveEthAddress(), demoEthAccounts()]);
    const balance = await ethBalance(address).then(String).catch(() => null);
    const info = { address, balance, shop: demo.shop.toLowerCase(), drainer: demo.drainer.toLowerCase() };
    ethCache = { at: Date.now(), info };
    return info;
  } catch {
    return ethCache?.info ?? null;
  }
}

/** Signed Ethereum requests get their Sepolia tx hash, computed from the stored signature. */
function withEthHash(r: SpendRequest): SpendRequest {
  if (r.chain !== "ethereum" || !r.eth?.signature) return r;
  try {
    return { ...r, eth_hash: signedRawTx(r.eth as Parameters<typeof signedRawTx>[0]).hash };
  } catch {
    return r;
  }
}

export async function GET() {
  try {
    const [vault, requests] = await Promise.all([viewVault(), viewRequests(40)]);
    if (!keysCache || Date.now() - keysCache.at > 60_000) {
      keysCache = { at: Date.now(), keys: await viewAgentKeys() };
    }
    const eth = vault.eth ? await ethInfo() : null;
    return NextResponse.json(
      { vault, requests: requests.map(withEthHash), agentKeys: keysCache.keys, eth, at: Date.now() },
      { headers: { "cache-control": "no-store" } },
    );
  } catch (e) {
    return NextResponse.json({ error: e instanceof Error ? e.message : "rpc_error" }, { status: 502 });
  }
}
