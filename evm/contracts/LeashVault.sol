// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {Ownable, Ownable2Step} from "@openzeppelin/contracts/access/Ownable2Step.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {EnumerableSet} from "@openzeppelin/contracts/utils/structs/EnumerableSet.sol";
import {EnumerableMap} from "@openzeppelin/contracts/utils/structs/EnumerableMap.sol";

/// @title Leash vault
/// @notice An on-chain spending policy for an AI agent. The vault holds one
/// ERC-20 token (USDC in practice). The agent can only call `requestSpend`,
/// and every rule below runs inside the contract, so a jailbroken or
/// prompt-injected agent still cannot move money outside the policy:
///
/// 1. Frozen vault            -> blocked
/// 2. Receiver on blocklist   -> blocked
/// 3. Not enough balance      -> blocked
/// 4. Over the per-payment cap, over the daily cap, over that receiver's own
///    daily limit, or a receiver that is not on the allowlist
///                            -> pending, until the owner approves or denies
///                               (blocked instead if the approval queue is full)
/// 5. Otherwise               -> paid immediately
///
/// A policy refusal never reverts. It is recorded as a request with a status
/// and a rule, so the full history can be read back from the chain.
///
/// Tripwire: if the vault blocks too many requests in a short window, it
/// freezes itself. An agent that keeps attacking is cut off without waiting
/// for the owner to notice.
///
/// @dev Port of the Leash NEAR contract (`contract/src/lib.rs`). Assumes a
/// standard ERC-20: no fee on transfer, no rebasing, no transfer hooks.
contract LeashVault is Ownable2Step, ReentrancyGuard {
    using SafeERC20 for IERC20;
    using EnumerableSet for EnumerableSet.AddressSet;
    using EnumerableMap for EnumerableMap.AddressToUintMap;

    // ---------------------------------------------------------------------
    // Types
    // ---------------------------------------------------------------------

    enum Status {
        /// Paid automatically because it was inside the policy.
        Executed,
        /// Waiting for the owner.
        Pending,
        /// Approved by the owner and paid.
        Approved,
        /// Rejected by the owner.
        Denied,
        /// Refused by the contract. Never paid.
        Blocked
    }

    /// @notice The rule that decided a request.
    /// @dev The last four are "hold" rules: they normally give `Pending`. A
    /// request that is `Blocked` with a hold rule was refused because the
    /// approval queue already held `MAX_OPEN_PENDING` requests.
    enum Rule {
        WithinPolicy,
        VaultFrozen,
        TripwireFrozen,
        ReceiverBlocklisted,
        InsufficientFunds,
        OverPerTxCap,
        OverDailyCap,
        OverReceiverCap,
        NewReceiver
    }

    struct Request {
        address receiver;
        uint40 createdAt;
        /// Zero while the request is pending.
        uint40 resolvedAt;
        Status status;
        Rule rule;
        uint256 amount;
    }

    /// @notice Freeze the vault after `maxBlocked` blocked requests within
    /// `windowSecs`. `maxBlocked == 0` means the tripwire is off.
    /// `cooldownSecs == 0` keeps the vault frozen until the owner unfreezes it.
    struct Tripwire {
        uint32 maxBlocked;
        uint64 windowSecs;
        uint64 cooldownSecs;
    }

    /// @notice Running totals since the vault was created.
    struct Stats {
        uint64 requests;
        /// Paid inside the policy or approved by the owner.
        uint64 paid;
        /// Ever sent to the owner for a decision.
        uint64 held;
        uint64 denied;
        uint64 blocked;
        uint64 tripwireTrips;
        uint256 amountPaid;
        uint256 amountDenied;
        uint256 amountBlocked;
    }

    struct ReceiverSpent {
        /// The daily window this total belongs to. A stale id counts as zero.
        uint64 windowId;
        uint256 spent;
    }

    // ---------------------------------------------------------------------
    // Constants
    // ---------------------------------------------------------------------

    uint256 public constant DAY = 1 days;
    uint256 public constant MAX_REASON_LENGTH = 280;
    uint256 public constant MAX_OPEN_PENDING = 25;
    uint256 public constant MAX_TRIPWIRE_COUNT = 20;
    uint256 public constant MAX_TRIPWIRE_SECS = 30 days;
    uint256 public constant MAX_PAGE = 100;

    // ---------------------------------------------------------------------
    // State
    // ---------------------------------------------------------------------

    /// @notice The only token this vault holds and pays out.
    IERC20 public immutable token;

    /// @notice The only address allowed to call `requestSpend`.
    address public agent;

    /// @notice Largest single payment that goes through without the owner.
    uint256 public perTxCap;

    /// @notice Most the vault pays per daily window (automatic and approved payments).
    uint256 public dailyCap;

    /// @notice Start of the current daily window, in seconds.
    uint64 public windowStartedAt;

    /// @notice Increases every time a new daily window starts.
    uint64 public windowId;

    /// @notice When the tripwire freeze ends. `type(uint64).max` means until
    /// the owner unfreezes. Zero or a time in the past means no freeze holds.
    uint64 public trippedUntil;

    /// @notice True only for a manual freeze. See `isFrozen` for the combined state.
    bool public frozenByOwner;

    /// @notice Requests currently waiting for the owner.
    uint32 public openPending;

    Tripwire public tripwire;
    Stats public stats;

    mapping(address => string) public blockReason;

    uint256 private _spentInWindow;
    Request[] private _requests;
    EnumerableSet.AddressSet private _allowlist;
    EnumerableSet.AddressSet private _blocklist;
    EnumerableMap.AddressToUintMap private _receiverCaps;
    mapping(address => ReceiverSpent) private _receiverSpent;

    /// @dev Timestamps of the most recent blocked requests, as a ring buffer.
    /// Only the last `_ringCount` entries before `_ringHead` are meaningful.
    uint64[20] private _blockRing;
    uint8 private _ringHead;
    uint8 private _ringCount;

    // ---------------------------------------------------------------------
    // Events
    // ---------------------------------------------------------------------

    event SpendExecuted(uint256 indexed id, address indexed receiver, uint256 amount, string reason);
    event SpendPending(uint256 indexed id, address indexed receiver, uint256 amount, Rule rule, string reason);
    event SpendBlocked(uint256 indexed id, address indexed receiver, uint256 amount, Rule rule, string reason);
    event SpendApproved(uint256 indexed id, address indexed receiver, uint256 amount);
    event SpendDenied(uint256 indexed id, address indexed receiver, uint256 amount);
    event TripwireTripped(uint32 blockedCount, uint64 until);
    event WindowStarted(uint64 indexed windowId, uint64 startedAt);
    event Frozen();
    event Unfrozen();
    event CapsSet(uint256 perTxCap, uint256 dailyCap);
    event TripwireSet(uint32 maxBlocked, uint64 windowSecs, uint64 cooldownSecs);
    event TripwireDisabled();
    event ReceiverCapSet(address indexed receiver, uint256 dailyCap);
    event ReceiverCapRemoved(address indexed receiver);
    event ReceiverAllowed(address indexed receiver);
    event ReceiverDisallowed(address indexed receiver);
    event ReceiverBlocked(address indexed receiver, string reason);
    event ReceiverUnblocked(address indexed receiver);
    event AgentSet(address indexed agent);
    event Withdrawn(address indexed to, uint256 amount);
    event TokenRescued(address indexed token, address indexed to, uint256 amount);

    // ---------------------------------------------------------------------
    // Errors
    // ---------------------------------------------------------------------

    error OnlyAgent();
    error ZeroAmount();
    error ZeroAddress();
    error InvalidReceiver();
    error InvalidToken();
    error ReasonTooLong();
    error NoSuchRequest();
    error NotPending();
    error VaultIsFrozen();
    error ReceiverIsBlocklisted();
    error NotEnoughFunds();
    error OutOfRange();
    error RenounceDisabled();
    error UseWithdraw();

    // ---------------------------------------------------------------------
    // Setup
    // ---------------------------------------------------------------------

    constructor(address owner_, address agent_, IERC20 token_, uint256 perTxCap_, uint256 dailyCap_) Ownable(owner_) {
        if (agent_ == address(0)) revert ZeroAddress();
        if (address(token_).code.length == 0) revert InvalidToken();
        token = token_;
        agent = agent_;
        perTxCap = perTxCap_;
        dailyCap = dailyCap_;
        windowStartedAt = uint64(block.timestamp);
        emit AgentSet(agent_);
        emit CapsSet(perTxCap_, dailyCap_);
        emit WindowStarted(0, uint64(block.timestamp));
    }

    // ---------------------------------------------------------------------
    // Agent
    // ---------------------------------------------------------------------

    /// @notice The only function the agent can call. Reverts only on malformed
    /// input. A policy refusal is recorded and returned, never reverted.
    /// @param receiver Who gets paid.
    /// @param amount Token amount, in the token's smallest unit.
    /// @param reason What the agent says the payment is for. Emitted, not stored.
    /// @return id The request id.
    /// @return status What the vault did.
    /// @return rule The rule that decided it.
    function requestSpend(address receiver, uint256 amount, string calldata reason)
        external
        nonReentrant
        returns (uint256 id, Status status, Rule rule)
    {
        if (msg.sender != agent) revert OnlyAgent();
        _checkInput(receiver, amount);
        if (bytes(reason).length > MAX_REASON_LENGTH) revert ReasonTooLong();
        _rollWindow();

        (status, rule) = _decide(receiver, amount);

        id = _requests.length;
        _requests.push(
            Request({
                receiver: receiver,
                createdAt: uint40(block.timestamp),
                resolvedAt: status == Status.Pending ? 0 : uint40(block.timestamp),
                status: status,
                rule: rule,
                amount: amount
            })
        );
        stats.requests += 1;

        if (status == Status.Executed) {
            stats.paid += 1;
            stats.amountPaid += amount;
            _recordSpend(receiver, amount);
            emit SpendExecuted(id, receiver, amount, reason);
            token.safeTransfer(receiver, amount);
        } else if (status == Status.Pending) {
            stats.held += 1;
            openPending += 1;
            emit SpendPending(id, receiver, amount, rule, reason);
        } else {
            stats.blocked += 1;
            stats.amountBlocked += amount;
            emit SpendBlocked(id, receiver, amount, rule, reason);
            // Requests refused because the vault is already frozen do not count.
            if (!isFrozen()) _noteBlockAndMaybeTrip();
        }
    }

    // ---------------------------------------------------------------------
    // Owner: decisions
    // ---------------------------------------------------------------------

    /// @notice Pay a pending request. Counts toward the daily totals.
    function approve(uint256 id) external onlyOwner nonReentrant {
        Request storage r = _takePending(id);
        if (isFrozen()) revert VaultIsFrozen();
        address receiver = r.receiver;
        uint256 amount = r.amount;
        if (_blocklist.contains(receiver)) revert ReceiverIsBlocklisted();
        if (amount > token.balanceOf(address(this))) revert NotEnoughFunds();
        _rollWindow();

        r.status = Status.Approved;
        r.resolvedAt = uint40(block.timestamp);
        stats.paid += 1;
        stats.amountPaid += amount;
        _recordSpend(receiver, amount);
        emit SpendApproved(id, receiver, amount);
        token.safeTransfer(receiver, amount);
    }

    /// @notice Reject a pending request. Works while the vault is frozen.
    function deny(uint256 id) external onlyOwner {
        Request storage r = _takePending(id);
        r.status = Status.Denied;
        r.resolvedAt = uint40(block.timestamp);
        stats.denied += 1;
        stats.amountDenied += r.amount;
        emit SpendDenied(id, r.receiver, r.amount);
    }

    /// @notice Kill switch: every agent request is blocked until `unfreeze`.
    function freeze() external onlyOwner {
        frozenByOwner = true;
        emit Frozen();
    }

    /// @notice Lifts both a manual freeze and a tripwire freeze.
    function unfreeze() external onlyOwner {
        frozenByOwner = false;
        trippedUntil = 0;
        _ringCount = 0;
        emit Unfrozen();
    }

    /// @notice Start a fresh daily window now.
    function resetWindow() external onlyOwner {
        _startWindow();
    }

    // ---------------------------------------------------------------------
    // Owner: policy
    // ---------------------------------------------------------------------

    function setCaps(uint256 perTxCap_, uint256 dailyCap_) external onlyOwner {
        perTxCap = perTxCap_;
        dailyCap = dailyCap_;
        emit CapsSet(perTxCap_, dailyCap_);
    }

    /// @notice Arm the tripwire: `maxBlocked` blocked requests within
    /// `windowSecs` freeze the vault. With `cooldownSecs` the freeze lifts by
    /// itself after that long. With zero, only the owner can unfreeze.
    function setTripwire(uint32 maxBlocked, uint64 windowSecs, uint64 cooldownSecs) external onlyOwner {
        if (maxBlocked == 0 || maxBlocked > MAX_TRIPWIRE_COUNT) revert OutOfRange();
        if (windowSecs == 0 || windowSecs > MAX_TRIPWIRE_SECS) revert OutOfRange();
        if (cooldownSecs > MAX_TRIPWIRE_SECS) revert OutOfRange();
        tripwire = Tripwire({maxBlocked: maxBlocked, windowSecs: windowSecs, cooldownSecs: cooldownSecs});
        _ringCount = 0;
        emit TripwireSet(maxBlocked, windowSecs, cooldownSecs);
    }

    /// @notice Turn the tripwire off. A freeze that already tripped stays
    /// until it cools down or the owner unfreezes.
    function disableTripwire() external onlyOwner {
        delete tripwire;
        _ringCount = 0;
        emit TripwireDisabled();
    }

    /// @notice Limit what one receiver can get per daily window, even a
    /// trusted one. Payments above it wait for the owner.
    function setReceiverCap(address receiver, uint256 dailyCap_) external onlyOwner {
        _receiverCaps.set(receiver, dailyCap_);
        emit ReceiverCapSet(receiver, dailyCap_);
    }

    function removeReceiverCap(address receiver) external onlyOwner {
        if (_receiverCaps.remove(receiver)) emit ReceiverCapRemoved(receiver);
    }

    /// @notice Trust a receiver. Also takes it off the blocklist.
    function allowReceiver(address receiver) external onlyOwner {
        if (receiver == address(0) || receiver == address(this)) revert InvalidReceiver();
        _unblock(receiver);
        if (_allowlist.add(receiver)) emit ReceiverAllowed(receiver);
    }

    function disallowReceiver(address receiver) external onlyOwner {
        if (_allowlist.remove(receiver)) emit ReceiverDisallowed(receiver);
    }

    /// @notice Never pay this receiver. Also takes it off the allowlist.
    function blockReceiver(address receiver, string calldata reason) external onlyOwner {
        if (bytes(reason).length > MAX_REASON_LENGTH) revert ReasonTooLong();
        if (_allowlist.remove(receiver)) emit ReceiverDisallowed(receiver);
        _blocklist.add(receiver);
        blockReason[receiver] = reason;
        emit ReceiverBlocked(receiver, reason);
    }

    function unblockReceiver(address receiver) external onlyOwner {
        _unblock(receiver);
    }

    /// @notice Rotate the agent, for example after its key leaks.
    function setAgent(address agent_) external onlyOwner {
        if (agent_ == address(0)) revert ZeroAddress();
        agent = agent_;
        emit AgentSet(agent_);
    }

    /// @notice Owner takes money back out. The agent has no path to this.
    function withdraw(uint256 amount) external onlyOwner nonReentrant {
        address to = owner();
        emit Withdrawn(to, amount);
        token.safeTransfer(to, amount);
    }

    /// @notice Recover any other token sent to the vault by mistake.
    function rescueToken(IERC20 other, uint256 amount) external onlyOwner nonReentrant {
        if (other == token) revert UseWithdraw();
        address to = owner();
        emit TokenRescued(address(other), to, amount);
        other.safeTransfer(to, amount);
    }

    /// @dev A vault without an owner could never be unfrozen or emptied.
    function renounceOwnership() public view override onlyOwner {
        revert RenounceDisabled();
    }

    // ---------------------------------------------------------------------
    // Views
    // ---------------------------------------------------------------------

    /// @notice Dry run: what `requestSpend` would decide for this receiver and
    /// amount right now. Same code path, changes nothing.
    function checkSpend(address receiver, uint256 amount) external view returns (Status status, Rule rule) {
        _checkInput(receiver, amount);
        return _decide(receiver, amount);
    }

    /// @notice Frozen by the owner, or held by the tripwire right now.
    function isFrozen() public view returns (bool) {
        return frozenByOwner || block.timestamp < trippedUntil;
    }

    /// @notice Paid in the current daily window. Zero once the window has expired.
    function spentToday() public view returns (uint256) {
        return _windowLive() ? _spentInWindow : 0;
    }

    /// @notice Paid to one receiver in the current daily window.
    function receiverSpentToday(address receiver) public view returns (uint256) {
        ReceiverSpent storage s = _receiverSpent[receiver];
        return (s.windowId == windowId && _windowLive()) ? s.spent : 0;
    }

    /// @notice Token balance the vault can pay from.
    function available() public view returns (uint256) {
        return token.balanceOf(address(this));
    }

    /// @notice Blocked requests counted toward the tripwire right now.
    function recentBlocked() external view returns (uint256 count) {
        uint64 windowSecs = tripwire.windowSecs;
        if (tripwire.maxBlocked == 0) return 0;
        for (uint256 k = 1; k <= _ringCount; ++k) {
            if (uint256(_ringAt(k)) + windowSecs <= block.timestamp) break;
            ++count;
        }
    }

    function requestCount() external view returns (uint256) {
        return _requests.length;
    }

    function getRequest(uint256 id) external view returns (Request memory) {
        if (id >= _requests.length) revert NoSuchRequest();
        return _requests[id];
    }

    /// @notice Newest first. `before` is an exclusive request id for paging.
    /// Pass `type(uint256).max` for the latest page. At most `MAX_PAGE` per call.
    function getRequests(uint256 before, uint256 limit) external view returns (Request[] memory page) {
        uint256 end = before < _requests.length ? before : _requests.length;
        if (limit > MAX_PAGE) limit = MAX_PAGE;
        uint256 n = end < limit ? end : limit;
        page = new Request[](n);
        for (uint256 i = 0; i < n; ++i) {
            page[i] = _requests[end - 1 - i];
        }
    }

    function isAllowed(address receiver) external view returns (bool) {
        return _allowlist.contains(receiver);
    }

    function isBlocked(address receiver) external view returns (bool) {
        return _blocklist.contains(receiver);
    }

    function allowlist() external view returns (address[] memory) {
        return _allowlist.values();
    }

    function blocklist() external view returns (address[] memory receivers, string[] memory reasons) {
        receivers = _blocklist.values();
        reasons = new string[](receivers.length);
        for (uint256 i = 0; i < receivers.length; ++i) {
            reasons[i] = blockReason[receivers[i]];
        }
    }

    /// @notice Every receiver-specific limit, with what that receiver got in the current window.
    function receiverCaps()
        external
        view
        returns (address[] memory receivers, uint256[] memory caps, uint256[] memory spent)
    {
        uint256 n = _receiverCaps.length();
        receivers = new address[](n);
        caps = new uint256[](n);
        spent = new uint256[](n);
        for (uint256 i = 0; i < n; ++i) {
            (receivers[i], caps[i]) = _receiverCaps.at(i);
            spent[i] = receiverSpentToday(receivers[i]);
        }
    }

    // ---------------------------------------------------------------------
    // Internal
    // ---------------------------------------------------------------------

    function _checkInput(address receiver, uint256 amount) private view {
        if (amount == 0) revert ZeroAmount();
        if (receiver == address(0) || receiver == address(this)) revert InvalidReceiver();
    }

    /// @dev The policy, in order. Shared by `requestSpend` and `checkSpend`.
    function _decide(address receiver, uint256 amount) private view returns (Status, Rule) {
        if (frozenByOwner) return (Status.Blocked, Rule.VaultFrozen);
        if (isFrozen()) return (Status.Blocked, Rule.TripwireFrozen);
        if (_blocklist.contains(receiver)) return (Status.Blocked, Rule.ReceiverBlocklisted);
        if (amount > token.balanceOf(address(this))) return (Status.Blocked, Rule.InsufficientFunds);
        if (amount > perTxCap) return _hold(Rule.OverPerTxCap);
        if (_exceeds(spentToday(), amount, dailyCap)) return _hold(Rule.OverDailyCap);
        (bool capped, uint256 cap) = _receiverCaps.tryGet(receiver);
        if (capped && _exceeds(receiverSpentToday(receiver), amount, cap)) return _hold(Rule.OverReceiverCap);
        if (!_allowlist.contains(receiver)) return _hold(Rule.NewReceiver);
        return (Status.Executed, Rule.WithinPolicy);
    }

    /// @dev True if `spent + amount > cap`, without overflow.
    function _exceeds(uint256 spent, uint256 amount, uint256 cap) private pure returns (bool) {
        return amount > cap || spent > cap - amount;
    }

    /// @dev A request that needs the owner waits in the queue, unless the queue is full.
    function _hold(Rule rule) private view returns (Status, Rule) {
        return (openPending >= MAX_OPEN_PENDING ? Status.Blocked : Status.Pending, rule);
    }

    function _takePending(uint256 id) private returns (Request storage r) {
        if (id >= _requests.length) revert NoSuchRequest();
        r = _requests[id];
        if (r.status != Status.Pending) revert NotPending();
        openPending -= 1;
    }

    function _windowLive() private view returns (bool) {
        return block.timestamp < uint256(windowStartedAt) + DAY;
    }

    function _rollWindow() private {
        if (!_windowLive()) _startWindow();
    }

    function _startWindow() private {
        windowId += 1;
        windowStartedAt = uint64(block.timestamp);
        _spentInWindow = 0;
        emit WindowStarted(windowId, uint64(block.timestamp));
    }

    /// @dev Call only after `_rollWindow`, so the current window is live.
    function _recordSpend(address receiver, uint256 amount) private {
        _spentInWindow += amount;
        _receiverSpent[receiver] = ReceiverSpent({windowId: windowId, spent: receiverSpentToday(receiver) + amount});
    }

    function _unblock(address receiver) private {
        if (_blocklist.remove(receiver)) {
            delete blockReason[receiver];
            emit ReceiverUnblocked(receiver);
        }
    }

    /// @dev Timestamp of the k-th most recent counted block, k >= 1.
    function _ringAt(uint256 k) private view returns (uint64) {
        uint256 size = _blockRing.length;
        return _blockRing[(uint256(_ringHead) + size - k) % size];
    }

    /// @dev Counts a blocked request and freezes the vault if this is the
    /// `maxBlocked`-th one inside the tripwire window.
    function _noteBlockAndMaybeTrip() private {
        Tripwire memory t = tripwire;
        if (t.maxBlocked == 0) return;

        // This block trips the wire if `maxBlocked - 1` earlier ones are still
        // inside the window. Timestamps only grow, so it is enough to look at
        // the oldest of those.
        uint256 needed = t.maxBlocked - 1;
        bool trip = needed == 0;
        if (!trip && _ringCount >= needed) {
            trip = uint256(_ringAt(needed)) + t.windowSecs > block.timestamp;
        }

        if (trip) {
            _ringCount = 0;
            uint64 until = t.cooldownSecs == 0 ? type(uint64).max : uint64(block.timestamp) + t.cooldownSecs;
            trippedUntil = until;
            stats.tripwireTrips += 1;
            emit TripwireTripped(t.maxBlocked, until);
        } else {
            uint256 size = _blockRing.length;
            _blockRing[_ringHead] = uint64(block.timestamp);
            _ringHead = uint8((uint256(_ringHead) + 1) % size);
            if (_ringCount < size) _ringCount += 1;
        }
    }
}
