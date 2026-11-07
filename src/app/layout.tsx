import type { Metadata, Viewport } from "next";
import { IBM_Plex_Mono, IBM_Plex_Sans } from "next/font/google";
import "./globals.css";
import { Providers } from "@/components/providers";

const plexMono = IBM_Plex_Mono({ subsets: ["latin"], weight: ["400", "500", "600"], variable: "--font-plex-mono" });
const plexSans = IBM_Plex_Sans({ subsets: ["latin"], weight: ["500", "600", "700"], variable: "--font-plex-sans" });

export const metadata: Metadata = {
  title: "Leash: your AI agent can spend, not steal",
  description:
    "Leash keeps an AI agent's budget in a NEAR smart contract. The agent's key can only ask to spend. The contract enforces caps, allowlists and owner approval, even if the agent is jailbroken.",
  openGraph: {
    title: "Leash: your AI agent can spend, not steal",
    description: "An on-chain spending policy for AI agents, built on NEAR. Try to jailbreak the agent.",
    type: "website",
  },
};

export const viewport: Viewport = { themeColor: "#f1ebdd", width: "device-width", initialScale: 1 };

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className={`${plexMono.variable} ${plexSans.variable}`}>
      <body className="font-sans">
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
