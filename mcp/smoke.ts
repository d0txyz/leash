/**
 * Talks to the Leash MCP server the way an agent would: over stdio, through
 * the official MCP client.
 *
 *   npm run mcp:smoke                       # read-only: tools, policy, dry runs, ledger
 *   npm run mcp:smoke -- spend books 0.4    # also one real request_spend (needs your agent key)
 */
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const transport = new StdioClientTransport({
  command: process.execPath,
  args: ["--import", "tsx", "mcp/leash-mcp.ts"],
  stderr: "inherit",
});
const client = new Client({ name: "leash-smoke", version: "1.0.0" });
await client.connect(transport);

async function call(name: string, args: Record<string, unknown> = {}) {
  const res = await client.callTool({ name, arguments: args });
  const body = (res.content as { type: string; text?: string }[]).map((c) => c.text ?? "").join("\n");
  console.log(`\n> ${name} ${JSON.stringify(args)}${res.isError ? "  (error)" : ""}\n${body}`);
}

const { tools } = await client.listTools();
console.log(`tools: ${tools.map((t) => t.name).join(", ")}`);
console.log(`instructions: ${client.getInstructions()}`);

await call("leash_policy");
for (const [receiver, amount_near] of [["cafe", "0.2"], ["cafe", "0.8"], ["cafee", "0.45"], ["drainer", "0.01"]]) {
  await call("leash_check", { receiver, amount_near });
}

const [cmd, receiver, amount] = process.argv.slice(2);
if (cmd === "spend" && receiver && amount) {
  await call("leash_spend", { receiver, amount_near: amount, reason: "MCP smoke test" });
}
await call("leash_requests", { limit: 3 });
await client.close();
