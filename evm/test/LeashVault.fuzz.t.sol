// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {LeashVault} from "../contracts/LeashVault.sol";
import {LeashBase} from "./Base.t.sol";

contract LeashVaultFuzzTest is LeashBase {
    function _receiver(uint256 seed) internal view returns (address) {
        address[4] memory all = [cafe, books, stranger, drainer];
        return all[seed % 4];
    }

    /// The dry run and the real call always agree, whatever came before.
    function testFuzz_CheckSpendMatchesRequestSpend(uint256[12] memory seeds, uint256[12] memory amounts) public {
        vm.startPrank(owner);
        vault.allowReceiver(books);
        vault.setReceiverCap(books, 12 * USDC);
        vault.setTripwire(4, 600, 300);
        vm.stopPrank();

        for (uint256 i = 0; i < seeds.length; ++i) {
            address receiver = _receiver(seeds[i]);
            uint256 amount = bound(amounts[i], 1, 25 * USDC);
            vm.warp(block.timestamp + (seeds[i] >> 8) % 6 hours);

            (LeashVault.Status s, LeashVault.Rule r) = vault.checkSpend(receiver, amount);
            _expect(receiver, amount, s, r);
        }
    }

    /// Without the owner, the agent cannot move more than the daily cap in
    /// one daily window, more than the per-payment cap at once, anything to
    /// a receiver that is not on the allowlist, or more than a receiver's own
    /// limit.
    function testFuzz_AgentAloneStaysInsideThePolicy(uint256[40] memory seeds, uint256[40] memory amounts) public {
        vm.startPrank(owner);
        vault.allowReceiver(books);
        vault.setReceiverCap(books, 12 * USDC);
        vm.stopPrank();

        uint256 paidThisWindow;
        uint256 booksThisWindow;
        uint64 window = vault.windowId();

        for (uint256 i = 0; i < seeds.length; ++i) {
            address receiver = _receiver(seeds[i]);
            uint256 amount = bound(amounts[i], 1, 25 * USDC);
            vm.warp(block.timestamp + (seeds[i] >> 8) % 3 hours);

            uint256 before = token.balanceOf(receiver);
            (, LeashVault.Status s,) = _spend(receiver, amount);
            uint256 moved = token.balanceOf(receiver) - before;

            if (vault.windowId() != window) {
                window = vault.windowId();
                paidThisWindow = 0;
                booksThisWindow = 0;
            }
            if (s == LeashVault.Status.Executed) {
                assertEq(moved, amount);
                assertLe(amount, PER_TX);
                assertTrue(receiver == cafe || receiver == books);
                paidThisWindow += amount;
                if (receiver == books) booksThisWindow += amount;
            } else {
                assertEq(moved, 0, "only an executed request moves money");
            }
            assertLe(paidThisWindow, DAILY);
            assertLe(booksThisWindow, 12 * USDC);
            assertEq(vault.spentToday(), paidThisWindow);
        }
        assertEq(token.balanceOf(stranger), 0);
        assertEq(token.balanceOf(drainer), 0);
    }

    /// The ring buffer tripwire behaves exactly like the plain list the NEAR
    /// contract keeps: count the blocks still inside the window, trip when
    /// this one makes `maxBlocked`.
    function testFuzz_TripwireMatchesAReferenceModel(uint8 maxBlocked, uint16 windowSecs, uint16[60] memory gaps)
        public
    {
        uint256 max = bound(maxBlocked, 1, 20);
        uint256 window = bound(windowSecs, 1, 2_000);
        vm.prank(owner);
        vault.setTripwire(uint32(max), uint64(window), 0);

        uint256[] memory live = new uint256[](0);
        for (uint256 i = 0; i < gaps.length; ++i) {
            vm.warp(block.timestamp + gaps[i] % 700);

            // Reference model.
            uint256 kept;
            uint256[] memory next = new uint256[](live.length + 1);
            for (uint256 j = 0; j < live.length; ++j) {
                if (live[j] + window > block.timestamp) next[kept++] = live[j];
            }
            next[kept++] = block.timestamp;
            bool trips = kept >= max;

            _blocked(drainer, 1, LeashVault.Rule.ReceiverBlocklisted);
            assertEq(vault.isFrozen(), trips, "trip decision");

            if (trips) {
                assertEq(vault.recentBlocked(), 0);
                vm.prank(owner);
                vault.unfreeze();
                live = new uint256[](0);
            } else {
                assertEq(vault.recentBlocked(), kept, "live block count");
                live = new uint256[](kept);
                for (uint256 j = 0; j < kept; ++j) {
                    live[j] = next[j];
                }
            }
        }
    }

    /// Paging returns every request exactly once, newest first.
    function testFuzz_PagingCoversTheLedger(uint8 count, uint8 pageSize) public {
        uint256 n = bound(count, 0, 40);
        uint256 size = bound(pageSize, 1, 15);
        for (uint256 i = 0; i < n; ++i) {
            _spend(cafe, i + 1);
        }
        uint256 expected = n;
        uint256 cursor = type(uint256).max;
        while (true) {
            LeashVault.Request[] memory page = vault.getRequests(cursor, size);
            if (page.length == 0) break;
            for (uint256 i = 0; i < page.length; ++i) {
                assertEq(page[i].amount, expected);
                expected -= 1;
            }
            cursor = expected;
        }
        assertEq(expected, 0);
    }
}
