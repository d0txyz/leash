// Client-safe Ethereum helpers (no viem), shared by the UI and the server.
export const ETH_EXPLORER = "https://sepolia.etherscan.io";
export const ethExplorerTx = (hash: string) => `${ETH_EXPLORER}/tx/${hash}`;
export const ethExplorerAddress = (a: string) => `${ETH_EXPLORER}/address/${a}`;

const WEI = 10n ** 18n;
/** wei -> "0.0005" with up to `digits` decimals, trailing zeros trimmed. */
export function fmtEth(wei: bigint | string, digits = 6): string {
  const v = BigInt(wei);
  const whole = v / WEI;
  const frac = (v % WEI).toString().padStart(18, "0").slice(0, digits).replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : whole.toString();
}

/** "0.0005" -> wei. */
export function toWei(eth: string): bigint {
  const [w, f = ""] = eth.trim().split(".");
  return BigInt(w || "0") * WEI + BigInt((f + "0".repeat(18)).slice(0, 18));
}

/** gwei, for fee caps. */
export function fmtGwei(wei: bigint | string): string {
  const v = Number(BigInt(wei)) / 1e9;
  return v >= 1 ? String(Math.round(v * 100) / 100) : v.toPrecision(2);
}
