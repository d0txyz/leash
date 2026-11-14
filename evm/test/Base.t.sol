// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {LeashVault} from "../contracts/LeashVault.sol";
import {MockUSDC} from "./mocks/Mocks.sol";

/// Shared setup: a vault holding 1,000 USDC, 10 per payment, 50 per day,
/// one trusted receiver (cafe) and one blocklisted receiver (drainer).
abstract contract LeashBase is Test {
    uint256 internal constant USDC = 1e6;
    uint256 internal constant PER_TX = 10 * USDC;
    uint256 internal constant DAILY = 50 * USDC;
    uint256 internal constant FUNDED = 1_000 * USDC;

    LeashVault internal vault;
    MockUSDC internal token;

    address internal owner = makeAddr("owner");
    address internal agent = makeAddr("agent");
    address internal cafe = makeAddr("cafe");
    address internal books = makeAddr("books");
    address internal stranger = makeAddr("stranger");
    address internal drainer = makeAddr("drainer");

    function setUp() public virtual {
        vm.warp(1_800_000_000);
        token = new MockUSDC();
        vault = new LeashVault(owner, agent, token, PER_TX, DAILY);
        token.mint(address(vault), FUNDED);
        vm.startPrank(owner);
        vault.allowReceiver(cafe);
        vault.blockReceiver(drainer, "known scam");
        vm.stopPrank();
    }

    function _spend(address receiver, uint256 amount)
        internal
        returns (uint256 id, LeashVault.Status status, LeashVault.Rule rule)
    {
        vm.prank(agent);
        return vault.requestSpend(receiver, amount, "test payment");
    }

    function _expect(address receiver, uint256 amount, LeashVault.Status status, LeashVault.Rule rule)
        internal
        returns (uint256 id)
    {
        LeashVault.Status gotStatus;
        LeashVault.Rule gotRule;
        (id, gotStatus, gotRule) = _spend(receiver, amount);
        assertEq(uint8(gotStatus), uint8(status), "status");
        assertEq(uint8(gotRule), uint8(rule), "rule");
    }

    function _paid(address receiver, uint256 amount) internal returns (uint256) {
        return _expect(receiver, amount, LeashVault.Status.Executed, LeashVault.Rule.WithinPolicy);
    }

    function _held(address receiver, uint256 amount, LeashVault.Rule rule) internal returns (uint256) {
        return _expect(receiver, amount, LeashVault.Status.Pending, rule);
    }

    function _blocked(address receiver, uint256 amount, LeashVault.Rule rule) internal returns (uint256) {
        return _expect(receiver, amount, LeashVault.Status.Blocked, rule);
    }

    function _status(uint256 id) internal view returns (LeashVault.Status) {
        return vault.getRequest(id).status;
    }
}
