import { NextResponse } from "next/server";
import { viewVault, viewRequests, viewAgentKeys, type AccessKeyInfo } from "@/lib/chain";

export const dynamic = "force-dynamic";

let keysCache: { at: number; keys: AccessKeyInfo[] } | null = null;

export async function GET() {
  try {
    const [vault, requests] = await Promise.all([viewVault(), viewRequests(40)]);
    if (!keysCache || Date.now() - keysCache.at > 60_000) {
      keysCache = { at: Date.now(), keys: await viewAgentKeys() };
    }
    return NextResponse.json(
      { vault, requests, agentKeys: keysCache.keys, at: Date.now() },
      { headers: { "cache-control": "no-store" } },
    );
  } catch (e) {
    return NextResponse.json({ error: e instanceof Error ? e.message : "rpc_error" }, { status: 502 });
  }
}
