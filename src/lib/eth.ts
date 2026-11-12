// Cross-chain side of Leash. The vault contract builds and hashes each
// Ethereum transaction and has NEAR's MPC network (Chain Signatures) sign it.
// This file only reads: the vault's derived address, balances, and turning a
// stored signature into a raw transaction anyone can broadcast.
import { baseDecode } from "near-api-js";
import { createPublicClient, http, keccak256, serializeTransaction, type Hex } from "viem";
import { publicKeyToAddress } from "viem/accounts";
import { sepolia } from "viem/chains";
import { getProvider, VAULT_ID } from "./chain";

export const MPC_CONTRACT = process.env.NEXT_PUBLIC_MPC_CONTRACT ?? "v1.signer-prod.testnet";
/** Must match ETH_PATH in contract/src/lib.rs. */
export const ETH_PATH = "leash-eth";
export const ETH_CHAIN_ID = sepolia.id;
export const ETH_CHAIN_NAME = "Ethereum Sepolia";
export const ETH_RPC_URL = process.env.ETH_RPC_URL ?? "https://ethereum-sepolia-rpc.publicnode.com";
export { ETH_EXPLORER, ethExplorerTx, ethExplorerAddress, fmtEth } from "./eth-links";

export interface EthSignature {
  big_r: string;
  s: string;
  recovery_id: number;
}

export interface EthTx {
  chain_id: number;
  to: string;
  value: string;
  gas_limit: number;
  max_fee_per_gas: string;
  max_priority_fee_per_gas: string;
  nonce: number | null;
  payload: string | null;
  signature: EthSignature | null;
}

export interface EthPolicy {
  chain_id: number;
  mpc_contract: string;
  per_tx_cap: string;
  daily_cap: string;
  max_fee_per_gas: string;
  max_priority_fee_per_gas: string;
  next_nonce: number;
}

let client: ReturnType<typeof createPublicClient> | null = null;
export function ethClient() {
  client ??= createPublicClient({ chain: sepolia, transport: http(ETH_RPC_URL) });
  return client;
}

const addressCache = new Map<string, Hex>();
/** The Ethereum address NEAR's MPC network derives for `predecessor` + ETH_PATH. */
export function deriveEthAddress(predecessor = VAULT_ID): Promise<Hex> {
  return derivePath(predecessor, ETH_PATH);
}

/**
 * Demo counterparties on Sepolia. Both are MPC-derived from the owner account,
 * so test ETH paid to them is not lost to a stranger.
 */
export async function demoEthAccounts(): Promise<{ shop: Hex; drainer: Hex }> {
  const root = VAULT_ID.replace(/^vault\./, "");
  const [shop, drainer] = await Promise.all([derivePath(root, "leash-shop"), derivePath(root, "leash-drainer")]);
  return { shop, drainer };
}

async function derivePath(predecessor: string, path: string): Promise<Hex> {
  const key = `${predecessor}/${path}`;
  const hit = addressCache.get(key);
  if (hit) return hit;
  const pk = (await getProvider().callFunction({
    contractId: MPC_CONTRACT,
    method: "derived_public_key",
    args: { path, predecessor, domain_id: 0 },
  })) as string;
  const address = publicKeyToAddress(`0x04${Buffer.from(baseDecode(pk.split(":")[1])).toString("hex")}` as Hex);
  addressCache.set(key, address);
  return address;
}

/** The vault's own unsigned transaction, as the contract hashed it. */
export function unsignedTx(tx: EthTx) {
  if (tx.nonce === null) throw new Error("not signed yet: no nonce");
  return {
    type: "eip1559" as const,
    chainId: tx.chain_id,
    nonce: tx.nonce,
    to: tx.to as Hex,
    value: BigInt(tx.value),
    gas: BigInt(tx.gas_limit),
    maxFeePerGas: BigInt(tx.max_fee_per_gas),
    maxPriorityFeePerGas: BigInt(tx.max_priority_fee_per_gas),
  };
}

/** Signed raw transaction and its hash, from the MPC signature stored on-chain. */
export function signedRawTx(tx: EthTx): { raw: Hex; hash: Hex } {
  const sig = tx.signature;
  if (!sig) throw new Error("no signature yet");
  const raw = serializeTransaction(unsignedTx(tx), {
    r: `0x${sig.big_r.slice(2)}`,
    s: `0x${sig.s}`,
    yParity: sig.recovery_id,
  });
  return { raw, hash: keccak256(raw) };
}

export type BroadcastResult = { hash: Hex; status: "sent" | "already_known" };

/** Sends a signed transaction. Anyone can do this; it changes nothing the policy decided. */
export async function broadcast(tx: EthTx): Promise<BroadcastResult> {
  const { raw, hash } = signedRawTx(tx);
  try {
    await ethClient().sendRawTransaction({ serializedTransaction: raw });
    return { hash, status: "sent" };
  } catch (e) {
    const msg = String((e as Error).message ?? e).toLowerCase();
    if (msg.includes("already known") || msg.includes("nonce too low") || msg.includes("already imported")) {
      return { hash, status: "already_known" };
    }
    throw e;
  }
}

export async function ethBalance(address: Hex) {
  return ethClient().getBalance({ address });
}

export async function ethNonce(address: Hex) {
  return ethClient().getTransactionCount({ address, blockTag: "pending" });
}

export async function ethReceipt(hash: Hex) {
  try {
    return await ethClient().getTransactionReceipt({ hash });
  } catch {
    return null;
  }
}

export type Settled =
  | { stage: "broadcast"; hash: Hex; nonce: number }
  | { stage: "unfunded" | "error" | "unsigned"; message: string; nonce?: number };

/**
 * After request_spend (or approve) on an Ethereum request: wait for the MPC
 * signature to land in the vault, then broadcast the signed transaction.
 */
export async function settleEthRequest(id: number, onSigned?: (nonce: number) => void): Promise<Settled> {
  const { viewRequest } = await import("./chain");
  let r = await viewRequest(id);
  for (let i = 0; i < 15 && r?.status === "Signing"; i++) {
    await new Promise((ok) => setTimeout(ok, 1500));
    r = await viewRequest(id);
  }
  const tx = r?.eth as EthTx | null | undefined;
  if (!tx?.signature || tx.nonce === null) {
    return { stage: "unsigned", message: "No signature from the MPC network yet. The vault rolls the spend back if it never comes." };
  }
  onSigned?.(tx.nonce);
  try {
    const b = await broadcast(tx);
    return { stage: "broadcast", hash: b.hash, nonce: tx.nonce };
  } catch (e) {
    const msg = String(e instanceof Error ? e.message : e);
    if (/insufficient funds/i.test(msg)) {
      return { stage: "unfunded", nonce: tx.nonce, message: "Signed, but the vault's Sepolia address is out of test ETH, so nothing was sent." };
    }
    return { stage: "error", nonce: tx.nonce, message: msg.slice(0, 200) };
  }
}

export async function isEthRequestId(id: number): Promise<boolean> {
  const { viewRequest } = await import("./chain");
  return (await viewRequest(id))?.chain === "ethereum";
}

export function isEthAddress(s: string) {
  return /^0x[0-9a-fA-F]{40}$/.test(s);
}
