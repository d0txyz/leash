import { runAgent, type AgentEvent, type ChatTurn } from "@/lib/agent";
import { maybeResetWindow } from "@/lib/demo-reset";

export const dynamic = "force-dynamic";
export const maxDuration = 60;

// Best-effort per-IP limit. The real spending limit is the vault contract.
const WINDOW_MS = 10 * 60 * 1000;
const MAX_RUNS = 15;
const hits = new Map<string, number[]>();

function limited(ip: string) {
  const now = Date.now();
  const recent = (hits.get(ip) ?? []).filter((t) => now - t < WINDOW_MS);
  recent.push(now);
  hits.set(ip, recent);
  return recent.length > MAX_RUNS;
}

export async function POST(req: Request) {
  const ip = req.headers.get("x-forwarded-for")?.split(",")[0]?.trim() || "local";
  if (limited(ip)) {
    return Response.json({ error: "Too many runs from your network. Try again in a few minutes." }, { status: 429 });
  }

  let body: { history?: unknown; jailbroken?: unknown };
  try {
    body = await req.json();
  } catch {
    return Response.json({ error: "Invalid JSON" }, { status: 400 });
  }
  const history = Array.isArray(body.history) ? body.history : [];
  const turns: ChatTurn[] = history
    .filter((t): t is ChatTurn => !!t && (t.role === "user" || t.role === "assistant") && typeof t.content === "string")
    .slice(-12)
    .map((t) => ({ role: t.role, content: t.content.slice(0, 4000) }));
  if (turns.at(-1)?.role !== "user" || !turns.at(-1)?.content.trim()) {
    return Response.json({ error: "Send a message for the agent." }, { status: 400 });
  }

  const encoder = new TextEncoder();
  const stream = new ReadableStream({
    async start(controller) {
      const emit = (e: AgentEvent) => controller.enqueue(encoder.encode(JSON.stringify(e) + "\n"));
      try {
        const reset = await maybeResetWindow();
        if (reset) {
          emit({
            type: "notice",
            text: "The shared daily allowance was nearly used up by an earlier visitor, so the demo owner started a fresh day with a key that can only call reset_window().",
            txHash: reset.txHash,
          });
        }
      } catch {
        // A failed reset never blocks the run; the contract still decides.
      }
      await runAgent(turns, body.jailbroken === true, emit);
      controller.close();
    },
  });
  return new Response(stream, {
    headers: { "content-type": "application/x-ndjson; charset=utf-8", "cache-control": "no-store" },
  });
}
