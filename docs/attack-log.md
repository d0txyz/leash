# Leash attack log

Run at 2026-11-06T17:44:33.253Z against `vault.leash-gv2adb.testnet` on NEAR testnet.
The attacker controls `agent.leash-gv2adb.testnet` and its only key. Vault free balance before: 9.953 NEAR.

## Part 1: go around the vault

### Transfer NEAR straight out of the agent account
- RESULT: protocol rejected: this action needs a full-access key, the agent has none

### Add a new full-access key to the agent account
- RESULT: protocol rejected: this action needs a full-access key, the agent has none

### Call vault.withdraw() to pull funds
- RESULT: protocol rejected: the agent key may only call request_spend(), not withdraw()

### Approve its own held request with vault.approve()
- RESULT: protocol rejected: the agent key may only call request_spend(), not approve()

### Put the attacker on the allowlist with vault.allow()
- RESULT: protocol rejected: the agent key may only call request_spend(), not allow()

## Part 2: go through the vault

### Pay the known drainer address
- request_spend(drainer, 1.5 NEAR) -> BLOCKED: Receiver is blocklisted
- tx: https://testnet.nearblocks.io/txns/3H2YqtWT6JByBt39HoyYTgdTUEFE1fbqbwhC7Zkhngok

### Pay a lookalike of a trusted merchant
- request_spend(cafee, 0.45 NEAR) -> HELD FOR OWNER: Receiver not on allowlist
- tx: https://testnet.nearblocks.io/txns/AEjZezZ3ox9hoR7NsNE7reGsfcY7GfwmpTsn2uooN5tJ

### Pay a trusted merchant, but over the per-payment cap
- request_spend(cafe, 2 NEAR) -> HELD FOR OWNER: Over the per-payment cap
- tx: https://testnet.nearblocks.io/txns/JCT7sufMzLzyGVTnV6EwhF1uxruk63DpAM87wR1yi9HF

### Pay more than the vault holds
- request_spend(cafe, 500 NEAR) -> BLOCKED: Not enough free funds
- tx: https://testnet.nearblocks.io/txns/A1aFptNpQ1Qn7YKcvoHvgGm2HLyjY5DnQSTXt2s42cqb

## Result

Vault free balance after: 9.947 NEAR. Requests waiting for the owner: 2.
Nothing reached the attacker. Held requests only move if the owner approves them from their own wallet.
