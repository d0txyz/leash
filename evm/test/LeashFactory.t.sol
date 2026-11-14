// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {LeashFactory} from "../contracts/LeashFactory.sol";
import {LeashVault} from "../contracts/LeashVault.sol";
import {MockUSDC} from "./mocks/Mocks.sol";

contract LeashFactoryTest is Test {
    LeashFactory internal factory;
    MockUSDC internal token;
    address internal alice = makeAddr("alice");
    address internal bob = makeAddr("bob");
    address internal agent = makeAddr("agent");

    function setUp() public {
        factory = new LeashFactory();
        token = new MockUSDC();
    }

    function test_CreatesAVaultOwnedByTheCaller() public {
        vm.prank(alice);
        LeashVault vault = factory.createVault(agent, token, 10e6, 50e6);

        assertEq(vault.owner(), alice);
        assertEq(vault.agent(), agent);
        assertEq(address(vault.token()), address(token));
        assertEq(vault.perTxCap(), 10e6);
        assertEq(vault.dailyCap(), 50e6);
    }

    function test_ListsVaultsPerCreator() public {
        vm.startPrank(alice);
        address first = address(factory.createVault(agent, token, 1, 2));
        address second = address(factory.createVault(agent, token, 3, 4));
        vm.stopPrank();
        vm.prank(bob);
        address third = address(factory.createVault(agent, token, 5, 6));

        address[] memory mine = factory.vaultsOf(alice);
        assertEq(mine.length, 2);
        assertEq(mine[0], first);
        assertEq(mine[1], second);
        assertEq(factory.vaultsOf(bob)[0], third);
        assertEq(factory.vaultsOf(agent).length, 0);
    }

    function test_TheFactoryHasNoPowerOverAVault() public {
        vm.prank(alice);
        LeashVault vault = factory.createVault(agent, token, 10e6, 50e6);
        token.mint(address(vault), 100e6);

        vm.prank(address(factory));
        vm.expectRevert();
        vault.withdraw(1);
        vm.prank(address(factory));
        vm.expectRevert(LeashVault.OnlyAgent.selector);
        vault.requestSpend(alice, 1, "");
    }

    function test_BadArgumentsRevert() public {
        vm.expectRevert(LeashVault.ZeroAddress.selector);
        factory.createVault(address(0), token, 1, 2);
    }
}
