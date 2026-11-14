// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {LeashVault} from "../contracts/LeashVault.sol";
import {LeashBase} from "./Base.t.sol";
import {MockUSDC, FalseReturnToken, ReentrantToken} from "./mocks/Mocks.sol";

contract LeashVaultTest is LeashBase {
    // ------------------------------------------------------------------
    // The policy
    // ------------------------------------------------------------------

    function test_PaysAllowlistedReceiverWithinCaps() public {
        vm.expectEmit(true, true, false, true, address(vault));
        emit LeashVault.SpendExecuted(0, cafe, 4 * USDC, "test payment");
        uint256 id = _paid(cafe, 4 * USDC);

        assertEq(token.balanceOf(cafe), 4 * USDC);
        assertEq(token.balanceOf(address(vault)), FUNDED - 4 * USDC);
        assertEq(vault.spentToday(), 4 * USDC);
        assertEq(vault.receiverSpentToday(cafe), 4 * USDC);
        LeashVault.Request memory r = vault.getRequest(id);
        assertEq(r.receiver, cafe);
        assertEq(r.amount, 4 * USDC);
        assertEq(r.createdAt, block.timestamp);
        assertEq(r.resolvedAt, block.timestamp);
    }

    function test_BlocksBlocklistedReceiverEvenForTinyAmounts() public {
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertEq(token.balanceOf(drainer), 0);
        assertEq(vault.spentToday(), 0);
    }

    function test_OverPerTxCapGoesToOwner() public {
        uint256 id = _held(cafe, PER_TX + 1, LeashVault.Rule.OverPerTxCap);
        assertEq(token.balanceOf(cafe), 0);
        assertEq(vault.openPending(), 1);
        assertEq(vault.getRequest(id).resolvedAt, 0);
        _paid(cafe, PER_TX);
    }

    function test_UnknownReceiverGoesToOwner() public {
        _held(stranger, 1 * USDC, LeashVault.Rule.NewReceiver);
        assertEq(token.balanceOf(stranger), 0);
    }

    function test_DailyCapHoldsAcrossManySmallPayments() public {
        for (uint256 i = 0; i < 5; ++i) {
            _paid(cafe, PER_TX);
        }
        assertEq(vault.spentToday(), DAILY);
        _held(cafe, 1, LeashVault.Rule.OverDailyCap);
        assertEq(token.balanceOf(cafe), DAILY);
    }

    function test_DailyWindowResetsAfter24h() public {
        for (uint256 i = 0; i < 5; ++i) {
            _paid(cafe, PER_TX);
        }
        vm.warp(block.timestamp + 1 days - 1);
        _held(cafe, 1, LeashVault.Rule.OverDailyCap);

        vm.warp(block.timestamp + 1);
        assertEq(vault.spentToday(), 0, "expired window reads as empty");
        _paid(cafe, PER_TX);
        assertEq(vault.windowId(), 1);
        assertEq(vault.windowStartedAt(), block.timestamp);
        assertEq(vault.spentToday(), PER_TX);
    }

    function test_FreezeBlocksEverything() public {
        vm.prank(owner);
        vault.freeze();
        assertTrue(vault.isFrozen());
        _blocked(cafe, 1, LeashVault.Rule.VaultFrozen);
        _blocked(drainer, 1, LeashVault.Rule.VaultFrozen);

        vm.prank(owner);
        vault.unfreeze();
        _paid(cafe, 1);
    }

    function test_CannotSpendMoreThanBalance() public {
        vm.prank(owner);
        vault.setCaps(type(uint256).max, type(uint256).max);
        _blocked(cafe, FUNDED + 1, LeashVault.Rule.InsufficientFunds);
        _paid(cafe, FUNDED);
        assertEq(token.balanceOf(address(vault)), 0);
    }

    function test_OwnerApprovesAndDenies() public {
        uint256 a = _held(stranger, 3 * USDC, LeashVault.Rule.NewReceiver);
        uint256 b = _held(stranger, 4 * USDC, LeashVault.Rule.NewReceiver);
        assertEq(vault.openPending(), 2);

        vm.warp(block.timestamp + 60);
        vm.expectEmit(true, true, false, true, address(vault));
        emit LeashVault.SpendApproved(a, stranger, 3 * USDC);
        vm.prank(owner);
        vault.approve(a);
        vm.prank(owner);
        vault.deny(b);

        assertEq(uint8(_status(a)), uint8(LeashVault.Status.Approved));
        assertEq(uint8(_status(b)), uint8(LeashVault.Status.Denied));
        assertEq(vault.getRequest(a).resolvedAt, block.timestamp);
        assertEq(token.balanceOf(stranger), 3 * USDC);
        assertEq(vault.openPending(), 0);
        assertEq(vault.spentToday(), 3 * USDC, "an approved payment counts toward the day");
        assertFalse(vault.isAllowed(stranger), "approving one payment does not trust the receiver");
    }

    function test_ApprovedPaymentsUseUpTheDailyCap() public {
        uint256 id = _held(cafe, DAILY, LeashVault.Rule.OverPerTxCap);
        vm.prank(owner);
        vault.approve(id);
        _held(cafe, 1, LeashVault.Rule.OverDailyCap);
    }

    function test_ApprovalCanExceedTheCapsAndTheMathStillHolds() public {
        uint256 id = _held(cafe, DAILY * 2, LeashVault.Rule.OverPerTxCap);
        vm.prank(owner);
        vault.approve(id);
        assertEq(vault.spentToday(), DAILY * 2);
        _held(cafe, 1, LeashVault.Rule.OverDailyCap);
    }

    function test_CannotApproveOrDenyTwice() public {
        uint256 id = _held(stranger, 1 * USDC, LeashVault.Rule.NewReceiver);
        vm.startPrank(owner);
        vault.approve(id);
        vm.expectRevert(LeashVault.NotPending.selector);
        vault.approve(id);
        vm.expectRevert(LeashVault.NotPending.selector);
        vault.deny(id);
        vm.expectRevert(LeashVault.NoSuchRequest.selector);
        vault.approve(99);
        vm.stopPrank();
        assertEq(token.balanceOf(stranger), 1 * USDC);
    }

    function test_CannotApproveAnExecutedOrBlockedRequest() public {
        uint256 paid = _paid(cafe, 1 * USDC);
        uint256 blocked = _blocked(drainer, 1 * USDC, LeashVault.Rule.ReceiverBlocklisted);
        vm.startPrank(owner);
        vm.expectRevert(LeashVault.NotPending.selector);
        vault.approve(paid);
        vm.expectRevert(LeashVault.NotPending.selector);
        vault.approve(blocked);
        vm.stopPrank();
    }

    function test_ApproveRevertsWhileFrozenButDenyWorks() public {
        uint256 a = _held(stranger, 1 * USDC, LeashVault.Rule.NewReceiver);
        uint256 b = _held(stranger, 1 * USDC, LeashVault.Rule.NewReceiver);
        vm.startPrank(owner);
        vault.freeze();
        vm.expectRevert(LeashVault.VaultIsFrozen.selector);
        vault.approve(a);
        vault.deny(b);
        vm.stopPrank();
        assertEq(vault.openPending(), 1, "the failed approval left the request pending");
    }

    function test_ApproveRevertsIfReceiverWasBlocklistedMeanwhile() public {
        uint256 id = _held(stranger, 1 * USDC, LeashVault.Rule.NewReceiver);
        vm.startPrank(owner);
        vault.blockReceiver(stranger, "turned out to be a scam");
        vm.expectRevert(LeashVault.ReceiverIsBlocklisted.selector);
        vault.approve(id);
        vault.deny(id);
        vm.stopPrank();
        assertEq(token.balanceOf(stranger), 0);
    }

    function test_ApproveRevertsWhenTheVaultCannotCoverIt() public {
        uint256 id = _held(stranger, 5 * USDC, LeashVault.Rule.NewReceiver);
        vm.startPrank(owner);
        vault.withdraw(FUNDED - 1 * USDC);
        vm.expectRevert(LeashVault.NotEnoughFunds.selector);
        vault.approve(id);
        vm.stopPrank();
        assertEq(uint8(_status(id)), uint8(LeashVault.Status.Pending));
    }

    function test_OpenPendingIsCapped() public {
        for (uint256 i = 0; i < 25; ++i) {
            _held(stranger, 1 * USDC, LeashVault.Rule.NewReceiver);
        }
        assertEq(vault.openPending(), 25);
        // A hold rule with status Blocked means the queue was full.
        _blocked(stranger, 1 * USDC, LeashVault.Rule.NewReceiver);
        _blocked(cafe, PER_TX + 1, LeashVault.Rule.OverPerTxCap);
        _paid(cafe, 1 * USDC);

        vm.prank(owner);
        vault.deny(0);
        _held(stranger, 1 * USDC, LeashVault.Rule.NewReceiver);
    }

    // ------------------------------------------------------------------
    // Per-receiver limits and the daily window
    // ------------------------------------------------------------------

    function test_ReceiverCapLimitsATrustedReceiver() public {
        vm.startPrank(owner);
        vault.allowReceiver(books);
        vault.setReceiverCap(books, 15 * USDC);
        vm.stopPrank();

        _paid(books, 10 * USDC);
        _held(books, 6 * USDC, LeashVault.Rule.OverReceiverCap);
        _paid(books, 5 * USDC);
        _paid(cafe, 10 * USDC);

        (address[] memory receivers, uint256[] memory caps, uint256[] memory spent) = vault.receiverCaps();
        assertEq(receivers.length, 1);
        assertEq(receivers[0], books);
        assertEq(caps[0], 15 * USDC);
        assertEq(spent[0], 15 * USDC);
    }

    function test_ZeroReceiverCapHoldsEveryPayment() public {
        vm.prank(owner);
        vault.setReceiverCap(cafe, 0);
        _held(cafe, 1, LeashVault.Rule.OverReceiverCap);
        vm.prank(owner);
        vault.removeReceiverCap(cafe);
        _paid(cafe, 1);
    }

    function test_ReceiverCapCountsSpendFromBeforeItWasSet() public {
        _paid(cafe, 10 * USDC);
        vm.prank(owner);
        vault.setReceiverCap(cafe, 15 * USDC);
        _held(cafe, 6 * USDC, LeashVault.Rule.OverReceiverCap);
    }

    function test_ReceiverCapClearsWithTheDailyWindow() public {
        vm.prank(owner);
        vault.setReceiverCap(cafe, 10 * USDC);
        _paid(cafe, 10 * USDC);
        _held(cafe, 1, LeashVault.Rule.OverReceiverCap);

        vm.warp(block.timestamp + 1 days);
        assertEq(vault.receiverSpentToday(cafe), 0);
        _paid(cafe, 10 * USDC);
    }

    function test_OwnerResetStartsAFreshDailyWindow() public {
        vm.prank(owner);
        vault.setReceiverCap(cafe, 10 * USDC);
        _paid(cafe, 10 * USDC);

        // Two resets in one block: the window id still moves, so the
        // per-receiver total cannot survive a reset with an equal timestamp.
        vm.startPrank(owner);
        vault.resetWindow();
        vault.resetWindow();
        vm.stopPrank();

        assertEq(vault.windowId(), 2);
        assertEq(vault.spentToday(), 0);
        assertEq(vault.receiverSpentToday(cafe), 0);
        _paid(cafe, 10 * USDC);
    }

    // ------------------------------------------------------------------
    // Dry run
    // ------------------------------------------------------------------

    function test_CheckSpendMatchesRequestSpendAndWritesNothing() public {
        address[4] memory receivers = [cafe, stranger, drainer, books];
        uint256[4] memory amounts = [uint256(1 * USDC), 1 * USDC, 1, PER_TX + 1];
        for (uint256 i = 0; i < 4; ++i) {
            uint256 countBefore = vault.requestCount();
            (LeashVault.Status s, LeashVault.Rule r) = vault.checkSpend(receivers[i], amounts[i]);
            assertEq(vault.requestCount(), countBefore);
            _expect(receivers[i], amounts[i], s, r);
        }
    }

    function test_CheckSpendSeesAnExpiredWindowAsFresh() public {
        for (uint256 i = 0; i < 5; ++i) {
            _paid(cafe, PER_TX);
        }
        (LeashVault.Status s,) = vault.checkSpend(cafe, 1);
        assertEq(uint8(s), uint8(LeashVault.Status.Pending));

        vm.warp(block.timestamp + 1 days);
        (s,) = vault.checkSpend(cafe, 1);
        assertEq(uint8(s), uint8(LeashVault.Status.Executed));
        assertEq(vault.windowId(), 0, "a view does not roll the window");
    }

    // ------------------------------------------------------------------
    // Tripwire
    // ------------------------------------------------------------------

    function test_TripwireFreezesAfterRepeatedAttacks() public {
        vm.prank(owner);
        vault.setTripwire(3, 600, 120);

        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertEq(vault.recentBlocked(), 2);
        assertFalse(vault.isFrozen());

        vm.expectEmit(false, false, false, true, address(vault));
        emit LeashVault.TripwireTripped(3, uint64(block.timestamp + 120));
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);

        assertTrue(vault.isFrozen());
        assertFalse(vault.frozenByOwner());
        assertEq(vault.trippedUntil(), block.timestamp + 120);
        assertEq(vault.recentBlocked(), 0);
        _blocked(cafe, 1, LeashVault.Rule.TripwireFrozen);
        (,,,,, uint64 trips,,,) = vault.stats();
        assertEq(trips, 1);
    }

    function test_TripwireWithoutCooldownNeedsTheOwner() public {
        vm.prank(owner);
        vault.setTripwire(2, 600, 0);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);

        assertEq(vault.trippedUntil(), type(uint64).max);
        vm.warp(block.timestamp + 3650 days);
        _blocked(cafe, 1, LeashVault.Rule.TripwireFrozen);

        vm.prank(owner);
        vault.unfreeze();
        assertEq(vault.trippedUntil(), 0);
        _paid(cafe, 1);
    }

    function test_TripwireCooldownLiftsByItself() public {
        vm.prank(owner);
        vault.setTripwire(2, 600, 120);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);

        vm.warp(block.timestamp + 119);
        _blocked(cafe, 1, LeashVault.Rule.TripwireFrozen);
        vm.warp(block.timestamp + 1);
        assertFalse(vault.isFrozen());
        _paid(cafe, 1);
    }

    function test_TripwireForgetsBlocksOutsideItsWindow() public {
        vm.prank(owner);
        vault.setTripwire(3, 600, 120);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);

        vm.warp(block.timestamp + 600);
        assertEq(vault.recentBlocked(), 0);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertFalse(vault.isFrozen());
        assertEq(vault.recentBlocked(), 1);
    }

    function test_TripwireWindowEdgeIsExclusive() public {
        vm.prank(owner);
        vault.setTripwire(2, 600, 120);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        vm.warp(block.timestamp + 599);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertTrue(vault.isFrozen(), "599 seconds apart is inside a 600 second window");
    }

    function test_TripwireOfOneTripsOnTheFirstBlock() public {
        vm.prank(owner);
        vault.setTripwire(1, 600, 120);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertTrue(vault.isFrozen());
    }

    function test_BlocksWhileFrozenDoNotCountTowardTheNextTrip() public {
        vm.prank(owner);
        vault.setTripwire(3, 600, 120);
        for (uint256 i = 0; i < 3; ++i) {
            _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        }
        for (uint256 i = 0; i < 5; ++i) {
            _blocked(drainer, 1, LeashVault.Rule.TripwireFrozen);
        }
        vm.warp(block.timestamp + 120);
        assertEq(vault.recentBlocked(), 0);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertFalse(vault.isFrozen());
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertTrue(vault.isFrozen());
        (,,,,, uint64 trips,,,) = vault.stats();
        assertEq(trips, 2);
    }

    function test_TripwireRingBufferWrapsCleanly() public {
        vm.prank(owner);
        vault.setTripwire(3, 10, 0);
        // Far more spaced-out blocks than the buffer holds (and more than its
        // uint8 counter could count): none of them trip, none of them revert.
        for (uint256 i = 0; i < 300; ++i) {
            vm.warp(block.timestamp + 11);
            _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
            assertFalse(vault.isFrozen());
        }
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertTrue(vault.isFrozen());
    }

    function test_TripwireAtItsLargestSetting() public {
        vm.prank(owner);
        vault.setTripwire(20, 30 days, 0);
        for (uint256 i = 0; i < 19; ++i) {
            _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        }
        assertFalse(vault.isFrozen());
        assertEq(vault.recentBlocked(), 19);
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertTrue(vault.isFrozen());
    }

    function test_NoTripwireMeansNoAutoFreeze() public {
        for (uint256 i = 0; i < 30; ++i) {
            _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        }
        assertFalse(vault.isFrozen());
        assertEq(vault.recentBlocked(), 0);
    }

    function test_DisablingTheTripwireForgetsCountedBlocks() public {
        vm.startPrank(owner);
        vault.setTripwire(2, 600, 0);
        vm.stopPrank();
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        vm.startPrank(owner);
        vault.disableTripwire();
        vault.setTripwire(2, 600, 0);
        vm.stopPrank();
        _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
        assertFalse(vault.isFrozen());
    }

    function test_TripwireRejectsOutOfRangeSettings() public {
        vm.startPrank(owner);
        vm.expectRevert(LeashVault.OutOfRange.selector);
        vault.setTripwire(0, 600, 0);
        vm.expectRevert(LeashVault.OutOfRange.selector);
        vault.setTripwire(21, 600, 0);
        vm.expectRevert(LeashVault.OutOfRange.selector);
        vault.setTripwire(3, 0, 0);
        vm.expectRevert(LeashVault.OutOfRange.selector);
        vault.setTripwire(3, 30 days + 1, 0);
        vm.expectRevert(LeashVault.OutOfRange.selector);
        vault.setTripwire(3, 600, 30 days + 1);
        vault.setTripwire(20, 30 days, 30 days);
        vm.stopPrank();
    }

    function test_AFullApprovalQueueFeedsTheTripwire() public {
        vm.prank(owner);
        vault.setTripwire(2, 600, 0);
        for (uint256 i = 0; i < 25; ++i) {
            _held(stranger, 1, LeashVault.Rule.NewReceiver);
        }
        _blocked(stranger, 1, LeashVault.Rule.NewReceiver);
        _blocked(stranger, 1, LeashVault.Rule.NewReceiver);
        assertTrue(vault.isFrozen());
    }

    // ------------------------------------------------------------------
    // Who may call what
    // ------------------------------------------------------------------

    function test_StrangersCannotRequest() public {
        vm.prank(stranger);
        vm.expectRevert(LeashVault.OnlyAgent.selector);
        vault.requestSpend(stranger, 1, "");
        vm.prank(owner);
        vm.expectRevert(LeashVault.OnlyAgent.selector);
        vault.requestSpend(cafe, 1, "");
    }

    function test_AgentCannotCallAnyOwnerFunction() public {
        uint256 id = _held(stranger, 1 * USDC, LeashVault.Rule.NewReceiver);
        bytes[] memory calls = new bytes[](19);
        calls[0] = abi.encodeCall(vault.approve, (id));
        calls[1] = abi.encodeCall(vault.deny, (id));
        calls[2] = abi.encodeCall(vault.freeze, ());
        calls[3] = abi.encodeCall(vault.unfreeze, ());
        calls[4] = abi.encodeCall(vault.resetWindow, ());
        calls[5] = abi.encodeCall(vault.setCaps, (type(uint256).max, type(uint256).max));
        calls[6] = abi.encodeCall(vault.setTripwire, (1, 1, 1));
        calls[7] = abi.encodeCall(vault.disableTripwire, ());
        calls[8] = abi.encodeCall(vault.setReceiverCap, (cafe, type(uint256).max));
        calls[9] = abi.encodeCall(vault.removeReceiverCap, (cafe));
        calls[10] = abi.encodeCall(vault.allowReceiver, (drainer));
        calls[11] = abi.encodeCall(vault.disallowReceiver, (cafe));
        calls[12] = abi.encodeCall(vault.blockReceiver, (cafe, ""));
        calls[13] = abi.encodeCall(vault.unblockReceiver, (drainer));
        calls[14] = abi.encodeCall(vault.setAgent, (agent));
        calls[15] = abi.encodeCall(vault.withdraw, (1));
        calls[16] = abi.encodeCall(vault.rescueToken, (IERC20(address(token)), 1));
        calls[17] = abi.encodeCall(vault.transferOwnership, (agent));
        calls[18] = abi.encodeCall(vault.renounceOwnership, ());

        for (uint256 i = 0; i < calls.length; ++i) {
            vm.prank(agent);
            (bool ok, bytes memory err) = address(vault).call(calls[i]);
            assertFalse(ok, "an owner function accepted the agent");
            assertEq(err, abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, agent));
        }
        assertEq(token.balanceOf(address(vault)), FUNDED);
    }

    function test_RotatingTheAgentCutsOffTheOldKey() public {
        address fresh = makeAddr("fresh agent");
        vm.prank(owner);
        vault.setAgent(fresh);

        vm.prank(agent);
        vm.expectRevert(LeashVault.OnlyAgent.selector);
        vault.requestSpend(cafe, 1, "");
        vm.prank(fresh);
        vault.requestSpend(cafe, 1, "");
        assertEq(token.balanceOf(cafe), 1);

        vm.prank(owner);
        vm.expectRevert(LeashVault.ZeroAddress.selector);
        vault.setAgent(address(0));
    }

    function test_OwnershipMovesInTwoStepsAndCannotBeRenounced() public {
        address heir = makeAddr("heir");
        vm.prank(owner);
        vault.transferOwnership(heir);
        assertEq(vault.owner(), owner, "nothing moves until the new owner accepts");

        vm.prank(stranger);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, stranger));
        vault.acceptOwnership();

        vm.prank(heir);
        vault.acceptOwnership();
        assertEq(vault.owner(), heir);

        vm.prank(heir);
        vm.expectRevert(LeashVault.RenounceDisabled.selector);
        vault.renounceOwnership();
    }

    // ------------------------------------------------------------------
    // Lists
    // ------------------------------------------------------------------

    function test_AllowAndBlockAreMutuallyExclusive() public {
        vm.startPrank(owner);
        vault.blockReceiver(cafe, "compromised");
        assertFalse(vault.isAllowed(cafe));
        assertTrue(vault.isBlocked(cafe));
        assertEq(vault.blockReason(cafe), "compromised");

        vault.allowReceiver(cafe);
        assertTrue(vault.isAllowed(cafe));
        assertFalse(vault.isBlocked(cafe));
        assertEq(vault.blockReason(cafe), "");

        vault.unblockReceiver(drainer);
        vault.disallowReceiver(cafe);
        vm.stopPrank();

        assertEq(vault.allowlist().length, 0);
        (address[] memory blocked, string[] memory reasons) = vault.blocklist();
        assertEq(blocked.length, 0);
        assertEq(reasons.length, 0);
        _held(drainer, 1, LeashVault.Rule.NewReceiver);
    }

    function test_ListsReadBack() public view {
        address[] memory allowed = vault.allowlist();
        assertEq(allowed.length, 1);
        assertEq(allowed[0], cafe);
        (address[] memory blocked, string[] memory reasons) = vault.blocklist();
        assertEq(blocked.length, 1);
        assertEq(blocked[0], drainer);
        assertEq(reasons[0], "known scam");
    }

    function test_CannotAllowTheVaultItselfOrTheZeroAddress() public {
        vm.startPrank(owner);
        vm.expectRevert(LeashVault.InvalidReceiver.selector);
        vault.allowReceiver(address(vault));
        vm.expectRevert(LeashVault.InvalidReceiver.selector);
        vault.allowReceiver(address(0));
        vm.stopPrank();
    }

    // ------------------------------------------------------------------
    // Malformed input
    // ------------------------------------------------------------------

    function test_MalformedRequestsRevertAndRecordNothing() public {
        vm.startPrank(agent);
        vm.expectRevert(LeashVault.ZeroAmount.selector);
        vault.requestSpend(cafe, 0, "");
        vm.expectRevert(LeashVault.InvalidReceiver.selector);
        vault.requestSpend(address(0), 1, "");
        vm.expectRevert(LeashVault.InvalidReceiver.selector);
        vault.requestSpend(address(vault), 1, "");
        vm.expectRevert(LeashVault.ReasonTooLong.selector);
        vault.requestSpend(cafe, 1, string(new bytes(281)));
        vault.requestSpend(cafe, 1, string(new bytes(280)));
        vm.stopPrank();
        assertEq(vault.requestCount(), 1);

        vm.expectRevert(LeashVault.ZeroAmount.selector);
        vault.checkSpend(cafe, 0);
        vm.expectRevert(LeashVault.InvalidReceiver.selector);
        vault.checkSpend(address(0), 1);
    }

    function test_BlockReasonLengthIsLimited() public {
        vm.prank(owner);
        vm.expectRevert(LeashVault.ReasonTooLong.selector);
        vault.blockReceiver(stranger, string(new bytes(281)));
    }

    function test_ConstructorRejectsBadArguments() public {
        vm.expectRevert(LeashVault.ZeroAddress.selector);
        new LeashVault(owner, address(0), token, PER_TX, DAILY);
        vm.expectRevert(LeashVault.InvalidToken.selector);
        new LeashVault(owner, agent, IERC20(makeAddr("not a contract")), PER_TX, DAILY);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableInvalidOwner.selector, address(0)));
        new LeashVault(address(0), agent, token, PER_TX, DAILY);
    }

    // ------------------------------------------------------------------
    // Money out
    // ------------------------------------------------------------------

    function test_OwnerWithdraws() public {
        vm.expectEmit(true, false, false, true, address(vault));
        emit LeashVault.Withdrawn(owner, 100 * USDC);
        vm.prank(owner);
        vault.withdraw(100 * USDC);
        assertEq(token.balanceOf(owner), 100 * USDC);
        assertEq(vault.available(), FUNDED - 100 * USDC);
        assertEq(vault.spentToday(), 0, "a withdrawal is not agent spending");
    }

    function test_OwnerRescuesAStrayTokenButNotTheVaultToken() public {
        MockUSDC stray = new MockUSDC();
        stray.mint(address(vault), 7);
        vm.startPrank(owner);
        vault.rescueToken(IERC20(address(stray)), 7);
        vm.expectRevert(LeashVault.UseWithdraw.selector);
        vault.rescueToken(IERC20(address(token)), 1);
        vm.stopPrank();
        assertEq(stray.balanceOf(owner), 7);
    }

    function test_ATransferThatReturnsFalseRevertsTheWholeRequest() public {
        FalseReturnToken bad = new FalseReturnToken();
        LeashVault v = new LeashVault(owner, agent, IERC20(address(bad)), PER_TX, DAILY);
        bad.mint(address(v), FUNDED);
        vm.prank(owner);
        v.allowReceiver(cafe);

        vm.prank(agent);
        vm.expectRevert(abi.encodeWithSelector(SafeERC20.SafeERC20FailedOperation.selector, address(bad)));
        v.requestSpend(cafe, 1, "");
        assertEq(v.requestCount(), 0);
        assertEq(v.spentToday(), 0);
    }

    function test_ATokenCannotReenterDuringAPayment() public {
        ReentrantToken evil = new ReentrantToken();
        // The token is the agent, so the inner call passes the agent check
        // and is stopped by the reentrancy guard alone.
        LeashVault v = new LeashVault(owner, address(evil), IERC20(address(evil)), PER_TX, DAILY);
        evil.mint(address(v), FUNDED);
        vm.prank(owner);
        v.allowReceiver(cafe);
        evil.arm(v, cafe);

        vm.expectRevert(ReentrancyGuard.ReentrancyGuardReentrantCall.selector);
        evil.attack(1 * USDC);
        assertEq(evil.balanceOf(cafe), 0);
        assertEq(v.requestCount(), 0);
    }

    // ------------------------------------------------------------------
    // Ledger
    // ------------------------------------------------------------------

    function test_StatsCountEveryOutcome() public {
        _paid(cafe, 2 * USDC);
        uint256 a = _held(stranger, 3 * USDC, LeashVault.Rule.NewReceiver);
        uint256 b = _held(stranger, 4 * USDC, LeashVault.Rule.NewReceiver);
        _held(stranger, 5 * USDC, LeashVault.Rule.NewReceiver);
        _blocked(drainer, 6 * USDC, LeashVault.Rule.ReceiverBlocklisted);
        vm.startPrank(owner);
        vault.approve(a);
        vault.deny(b);
        vm.stopPrank();

        (
            uint64 requests,
            uint64 paid,
            uint64 held,
            uint64 denied,
            uint64 blocked,
            uint64 trips,
            uint256 amountPaid,
            uint256 amountDenied,
            uint256 amountBlocked
        ) = vault.stats();
        assertEq(requests, 5);
        assertEq(paid, 2);
        assertEq(held, 3);
        assertEq(denied, 1);
        assertEq(blocked, 1);
        assertEq(trips, 0);
        assertEq(amountPaid, 5 * USDC);
        assertEq(amountDenied, 4 * USDC);
        assertEq(amountBlocked, 6 * USDC);
        assertEq(vault.openPending(), 1);
    }

    function test_NewestRequestsFirstWithPaging() public {
        for (uint256 i = 1; i <= 5; ++i) {
            _paid(cafe, i);
        }
        LeashVault.Request[] memory page = vault.getRequests(type(uint256).max, 2);
        assertEq(page.length, 2);
        assertEq(page[0].amount, 5);
        assertEq(page[1].amount, 4);

        page = vault.getRequests(3, 10);
        assertEq(page.length, 3);
        assertEq(page[0].amount, 3);
        assertEq(page[2].amount, 1);

        assertEq(vault.getRequests(0, 10).length, 0);
        assertEq(vault.getRequests(type(uint256).max, 0).length, 0);
        assertEq(vault.getRequests(type(uint256).max, 1_000).length, 5);

        vm.expectRevert(LeashVault.NoSuchRequest.selector);
        vault.getRequest(5);
    }
}
