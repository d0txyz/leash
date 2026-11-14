// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {LeashVault} from "../../contracts/LeashVault.sol";

/// A plain 6-decimal token standing in for USDC.
contract MockUSDC is ERC20 {
    constructor() ERC20("USD Coin", "USDC") {}

    function decimals() public pure override returns (uint8) {
        return 6;
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

/// A token whose `transfer` reports failure by returning false.
contract FalseReturnToken {
    mapping(address => uint256) public balanceOf;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function transfer(address, uint256) external pure returns (bool) {
        return false;
    }
}

/// A token that is also the vault's agent, and tries to request a second
/// payment from inside the first payment's transfer.
contract ReentrantToken is ERC20 {
    LeashVault public vault;
    address public victim;
    bool public armed;

    constructor() ERC20("Reentrant", "RE") {}

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function arm(LeashVault vault_, address victim_) external {
        vault = vault_;
        victim = victim_;
        armed = true;
    }

    function attack(uint256 amount) external {
        vault.requestSpend(victim, amount, "first");
    }

    function _update(address from, address to, uint256 value) internal override {
        super._update(from, to, value);
        if (armed && from == address(vault)) {
            armed = false;
            vault.requestSpend(victim, value, "again, from inside the transfer");
        }
    }
}
