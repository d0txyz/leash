// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {LeashVault} from "../contracts/LeashVault.sol";
import {MockUSDC} from "./mocks/Mocks.sol";

/// Drives the vault with random agent requests, owner decisions, freezes,
/// withdrawals and time jumps, and keeps its own books to compare against.
contract Handler is Test {
    uint256 internal constant USDC = 1e6;

    LeashVault public vault;
    MockUSDC public token;
    address public owner;
    address public agent;
    address[4] public receivers;

    /// Paid without the owner in the vault's current window.
    uint256 public autoPaidInWindow;
    uint64 public autoWindow;
    uint256 public withdrawn;
    bool public autoPaymentBrokeARule;

    constructor(LeashVault vault_, MockUSDC token_, address owner_, address agent_, address[4] memory receivers_) {
        vault = vault_;
        token = token_;
        owner = owner_;
        agent = agent_;
        receivers = receivers_;
    }

    function request(uint256 receiverSeed, uint256 amount) external {
        address receiver = receivers[receiverSeed % 4];
        amount = bound(amount, 1, 25 * USDC);
        bool trusted = vault.isAllowed(receiver);
        vm.prank(agent);
        (, LeashVault.Status status,) = vault.requestSpend(receiver, amount, "");
        if (status != LeashVault.Status.Executed) return;

        if (vault.windowId() != autoWindow) {
            autoWindow = vault.windowId();
            autoPaidInWindow = 0;
        }
        autoPaidInWindow += amount;
        if (amount > vault.perTxCap() || !trusted) autoPaymentBrokeARule = true;
    }

    function approve(uint256 idSeed) external {
        uint256 count = vault.requestCount();
        if (count == 0) return;
        vm.prank(owner);
        try vault.approve(idSeed % count) {} catch {}
    }

    function deny(uint256 idSeed) external {
        uint256 count = vault.requestCount();
        if (count == 0) return;
        vm.prank(owner);
        try vault.deny(idSeed % count) {} catch {}
    }

    function warp(uint256 secs) external {
        vm.warp(block.timestamp + bound(secs, 1, 9 hours));
    }

    function freeze() external {
        vm.prank(owner);
        vault.freeze();
    }

    function unfreeze() external {
        vm.prank(owner);
        vault.unfreeze();
    }

    function resetWindow() external {
        vm.prank(owner);
        vault.resetWindow();
    }

    function withdraw(uint256 amount) external {
        amount = bound(amount, 0, vault.available() / 20);
        vm.prank(owner);
        vault.withdraw(amount);
        withdrawn += amount;
    }
}

contract LeashVaultInvariantTest is Test {
    uint256 internal constant USDC = 1e6;
    uint256 internal constant PER_TX = 10 * USDC;
    uint256 internal constant DAILY = 50 * USDC;
    uint256 internal constant FUNDED = 1_000 * USDC;

    LeashVault internal vault;
    MockUSDC internal token;
    Handler internal handler;

    address internal owner = makeAddr("owner");
    address internal agent = makeAddr("agent");
    address internal cafe = makeAddr("cafe");
    address internal books = makeAddr("books");
    address internal stranger = makeAddr("stranger");
    address internal drainer = makeAddr("drainer");

    function setUp() public {
        vm.warp(1_800_000_000);
        token = new MockUSDC();
        vault = new LeashVault(owner, agent, token, PER_TX, DAILY);
        token.mint(address(vault), FUNDED);
        vm.startPrank(owner);
        vault.allowReceiver(cafe);
        vault.allowReceiver(books);
        vault.setReceiverCap(books, 12 * USDC);
        vault.blockReceiver(drainer, "known scam");
        vault.setTripwire(5, 600, 300);
        vm.stopPrank();

        handler = new Handler(vault, token, owner, agent, [cafe, books, stranger, drainer]);
        targetContract(address(handler));
    }

    /// Whatever the agent does, payments made without the owner stay inside
    /// the caps and only reach trusted receivers.
    function invariant_AutoPaymentsStayInsideThePolicy() public view {
        assertLe(handler.autoPaidInWindow(), DAILY);
        assertFalse(handler.autoPaymentBrokeARule());
    }

    /// A blocklisted receiver never gets a token, even through an approval.
    function invariant_BlocklistedReceiverIsNeverPaid() public view {
        assertEq(token.balanceOf(drainer), 0);
    }

    /// Every token is in the vault, with a receiver, or back with the owner.
    function invariant_TokensAreConserved() public view {
        uint256 paidOut = token.balanceOf(cafe) + token.balanceOf(books) + token.balanceOf(stranger);
        assertEq(token.balanceOf(address(vault)) + paidOut + handler.withdrawn(), FUNDED);
        assertEq(token.balanceOf(owner), handler.withdrawn());
        (,,,,,, uint256 amountPaid,,) = vault.stats();
        assertEq(amountPaid, paidOut);
    }

    /// The counters agree with the ledger.
    function invariant_CountersMatchTheLedger() public view {
        uint256 count = vault.requestCount();
        uint256 pending;
        uint256 paid;
        uint256 denied;
        uint256 blocked;
        for (uint256 i = 0; i < count; ++i) {
            LeashVault.Status s = vault.getRequest(i).status;
            if (s == LeashVault.Status.Pending) pending++;
            else if (s == LeashVault.Status.Denied) denied++;
            else if (s == LeashVault.Status.Blocked) blocked++;
            else paid++;
        }
        (uint64 sRequests, uint64 sPaid, uint64 sHeld, uint64 sDenied, uint64 sBlocked,,,,) = vault.stats();
        assertEq(vault.openPending(), pending);
        assertLe(pending, vault.MAX_OPEN_PENDING());
        assertEq(sRequests, count);
        assertEq(sPaid, paid);
        assertEq(sDenied, denied);
        assertEq(sBlocked, blocked);
        assertGe(sHeld, pending + denied);
    }
}
