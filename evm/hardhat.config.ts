import { defineConfig } from "hardhat/config";

export default defineConfig({
  solidity: {
    version: "0.8.28",
    settings: {
      optimizer: { enabled: true, runs: 200 },
      evmVersion: "cancun",
    },
  },
  test: {
    solidity: {
      fuzz: { runs: 512 },
      invariant: { runs: 64, depth: 100, failOnRevert: false },
    },
  },
});
