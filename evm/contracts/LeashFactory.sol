// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {LeashVault} from "./LeashVault.sol";

/// @title Leash factory
/// @notice Deploys one `LeashVault` per call, owned by the caller, and keeps
/// a list so an app can find the vaults a wallet created.
contract LeashFactory {
    event VaultCreated(address indexed creator, address indexed vault, address indexed token, address agent);

    mapping(address => address[]) private _vaults;

    /// @notice Deploy a vault owned by the caller.
    /// @param agent The only address that may request payments from the vault.
    /// @param token The ERC-20 the vault holds, for example USDC.
    /// @param perTxCap Largest single payment that goes through without the owner.
    /// @param dailyCap Most the vault pays per daily window.
    function createVault(address agent, IERC20 token, uint256 perTxCap, uint256 dailyCap)
        external
        returns (LeashVault vault)
    {
        vault = new LeashVault(msg.sender, agent, token, perTxCap, dailyCap);
        _vaults[msg.sender].push(address(vault));
        emit VaultCreated(msg.sender, address(vault), address(token), agent);
    }

    /// @notice Vaults this wallet created here. Ownership can move later, so
    /// check `owner()` on the vault for the current owner.
    function vaultsOf(address creator) external view returns (address[] memory) {
        return _vaults[creator];
    }
}
