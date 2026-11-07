// Human labels for the contract's machine-readable rule strings. Shared by server and client.
const RULES: Record<string, string> = {
  within_policy: "inside policy",
  over_per_tx_cap: "over the per-payment cap",
  over_daily_cap: "over the daily cap",
  over_receiver_cap: "over this receiver's daily limit",
  new_receiver: "receiver not on allowlist",
  receiver_blocklisted: "receiver is blocklisted",
  vault_frozen: "vault is frozen",
  tripwire_frozen: "frozen by the tripwire",
  tripwire_tripped: "tripwire tripped, vault froze itself",
  insufficient_funds: "not enough free funds",
  too_many_open_requests: "too many open requests",
  approved_by_owner: "approved by owner",
  denied_by_owner: "denied by owner",
  transfer_failed: "transfer bounced",
};

/** "over_per_tx_cap -> approved_by_owner" => "Over the per-payment cap, approved by owner" */
export function ruleLabel(rule: string) {
  const text = rule
    .split(/\s*->\s*|;\s*/)
    .map((part) => RULES[part.split(":")[0].trim()] ?? part)
    .join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}
