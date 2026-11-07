"use client";

import { NearProvider } from "near-connect-hooks";

export function Providers({ children }: { children: React.ReactNode }) {
  return <NearProvider config={{ network: "testnet" }}>{children}</NearProvider>;
}
