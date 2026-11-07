// Read-only chain helpers. Safe on server and client.
import { JsonRpcProvider } from "near-api-js";

export const NETWORK = process.env.NEXT_PUBLIC_NEAR_NETWORK ?? "testnet";
export const RPC_URL = process.env.NEXT_PUBLIC_NEAR_RPC_URL ?? "https://test.rpc.fastnear.com";
export const VAULT_ID = process.env.NEXT_PUBLIC_VAULT_ID ?? "";
export const AGENT_ID = process.env.NEXT_PUBLIC_AGENT_ID ?? "";

const EXPLORER = NETWORK === "mainnet" ? "https://nearblocks.io" : "https://testnet.nearblocks.io";
export const explorerTx = (hash: string) => `${EXPLORER}/txns/${hash}`;
export const explorerAccount = (id: string) => `${EXPLORER}/address/${id}`;

export type Status = "Executed" | "Pending" | "Approved" | "Denied" | "Blocked" | "Failed";

export interface SpendRequest {
  id: number;
  receiver: string;
  amount: string; // yocto
  reason: string;
  status: Status;
  rule: string;
  created_at: number; // ns
  resolved_at: number | null;
}

export interface Vault {
  owner: string;
  agent: string;
  per_tx_cap: string;
  daily_cap: string;
  spent_today: string;
  window_started_at: number;
  frozen: boolean;
  balance: string;
  available: string;
  allowlist: string[];
  blocklist: [string, string][];
  open_pending: number;
  total_requests: number;
  /** Per-receiver daily limits. Missing on a vault that has not been migrated. */
  receiver_caps?: { receiver: string; daily_cap: string; spent_today: string }[];
  // v1.2: tripwire and on-chain stats. Optional so an older vault still renders.
  frozen_by_owner?: boolean;
  /** ns timestamp while the tripwire holds; "18446744073709551615" style max means until the owner unfreezes. */
  tripped_until?: number | null;
  tripwire?: { max_blocked: number; window_secs: number; cooldown_secs: number | null } | null;
  recent_blocked?: number;
  stats?: VaultStats;
}

export interface VaultStats {
  requests: number;
  paid: number;
  near_paid: string;
  held: number;
  denied: number;
  near_denied: string;
  blocked: number;
  near_blocked: string;
  failed: number;
  tripwire_trips: number;
}

/** True when the tripwire freeze has no end time (the contract stores u64::MAX). */
export function trippedForever(until: number | null | undefined) {
  return typeof until === "number" && until >= 1e19;
}

/** What check_spend says the vault would do right now. */
export interface Decision {
  status: Status;
  rule: string;
}

export interface AccessKeyInfo {
  public_key: string;
  permission: "FullAccess" | { FunctionCall: { allowance: string | null; method_names: string[]; receiver_id: string } };
}

let provider: JsonRpcProvider | null = null;
export function getProvider() {
  provider ??= new JsonRpcProvider({ url: RPC_URL });
  return provider;
}

export async function viewVault(): Promise<Vault> {
  return (await getProvider().callFunction({ contractId: VAULT_ID, method: "get_vault", args: {} })) as Vault;
}

export async function viewRequests(limit = 30): Promise<SpendRequest[]> {
  return (await getProvider().callFunction({
    contractId: VAULT_ID,
    method: "get_requests",
    args: { limit },
  })) as SpendRequest[];
}

/** Free dry run of request_spend: same rules, no transaction, nothing written. */
export async function viewCheckSpend(receiver: string, amountYocto: bigint | string): Promise<Decision> {
  return (await getProvider().callFunction({
    contractId: VAULT_ID,
    method: "check_spend",
    args: { receiver, amount: amountYocto.toString() },
  })) as Decision;
}

export async function viewRequest(id: number): Promise<SpendRequest | null> {
  return (await getProvider().callFunction({ contractId: VAULT_ID, method: "get_request", args: { id } })) as SpendRequest | null;
}

/** "cafe" -> "cafe.<root>" for the demo's short names; full account ids pass through. */
export function resolveAccount(input: string): string {
  const id = input.trim().toLowerCase();
  if (!id || id.includes(".") || /^[0-9a-f]{64}$/.test(id)) return id;
  const root = VAULT_ID.replace(/^vault\./, "");
  return root && root !== VAULT_ID ? `${id}.${root}` : id;
}

export function isAccountId(id: string): boolean {
  return id.length >= 2 && id.length <= 64 && /^(([a-z\d]+[-_])*[a-z\d]+\.)*([a-z\d]+[-_])*[a-z\d]+$/.test(id);
}

export async function viewAgentKeys(): Promise<AccessKeyInfo[]> {
  const res = await getProvider().viewAccessKeyList({ accountId: AGENT_ID });
  return res.keys.map((k) => ({ public_key: String(k.public_key), permission: k.access_key.permission as AccessKeyInfo["permission"] }));
}

const YOCTO = 10n ** 24n;
/** yocto string -> human NEAR string with up to 4 decimals */
export function fmtNear(yocto: string | bigint, digits = 2): string {
  const v = BigInt(yocto);
  const whole = v / YOCTO;
  const frac = (v % YOCTO).toString().padStart(24, "0").slice(0, digits).replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : whole.toString();
}

export function toYocto(near: string | number): bigint {
  const [w, f = ""] = String(near).trim().split(".");
  return BigInt(w || "0") * YOCTO + BigInt((f + "0".repeat(24)).slice(0, 24));
}

export function shortAccount(id: string) {
  return id.replace(/\.leash-[a-z0-9]+\.testnet$/, "");
}
