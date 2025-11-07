# Confidential-transfer

This repository contains a minimal Rust client that demonstrates using the Token-2022 confidential transfer extensions on a local Solana cluster. The code exercises mint creation, associated token account (ATA) configuration for confidential transfers, deposits (normal -> confidential), applying pending balances, creating withdraw proofs, performing confidential withdraws, and cleaning up proof accounts.

This document is strictly technical: architecture, build/run steps, key modules, data shapes, security notes, testing and troubleshooting information.

## Repository layout

- `Cargo.toml` — Cargo manifest with crates used by the client.
- `src/main.rs` — CLI-style example runner that executes an end-to-end confidential transfer flow against an RPC endpoint (default `http://localhost:8899`).
- `src/mint.rs` — Encapsulates mint creation and ATA configuration for confidential transfers.
- `src/utils.rs` — Small helper(s) (e.g. loading a Solana keypair).

## High-level architecture

- Single binary client (Rust / tokio async) that talks to a Solana JSON-RPC node via `solana-client` (`RpcClient` non-blocking).
- Uses `spl-token-client` (Token-2022 client), the `spl-token-confidential-transfer-proof-generation` crate for generating withdraw proof data, and `spl-token-confidential-transfer-proof-extraction` for any proof location helpers.
- The client performs the following logical steps:
  1. Load payer keypair from local disk (default Solana CLI location: `~/.config/solana/id.json`).
  2. Create a Token-2022 mint with the `ConfidentialTransferMint` extension.
  3. Create and reallocate an associated token account (ATA) for the payer to include the `ConfidentialTransferAccount` extension.
  4. Generate account-level crypto material (ElGamal keypair + AES key) derived from the ATA and payer (client-side) used for encrypting balances and generating proofs.
  5. Mint normal (transparent) tokens to the ATA.
  6. Deposit from the transparent ATA into the confidential state (pending balance).
  7. Apply pending balance to make confidential funds available (uses AES decryption and ElGamal secret keys locally).
  8. Prepare withdraw proof data (equality + range proofs), upload proof context accounts, run the confidential withdraw instruction, and close proof context accounts to reclaim rent.

All heavy cryptographic proof generation is performed client-side by the proof generation crate and ElGamal/AES key primitives. On-chain instructions verify proofs.

## Key files and responsibilities

- `src/main.rs`:

  - Builds an async `RpcClient` using `solana_client::nonblocking`.
  - Loads payer via `utils::load_keypair()`.
  - Calls `mint::initialize_mint()` which returns a newly created mint keypair and a `Token<ProgramRpcClientSendTransaction>` handle.
  - Calls `mint::create_configure_ata()` which creates the associated token account, reallocates it to include the confidential transfer extension, generates ElGamal/AES keys, and performs the on-chain `configure_account` sequence.
  - Executes a sequence of token operations via the `token` handle: `mint_to`, `confidential_transfer_deposit`, `confidential_transfer_apply_pending_balance`.
  - For withdraw: retrieves the `ConfidentialTransferAccount` extension from the token account, constructs `WithdrawAccountInfo`, generates `WithdrawProofData` for the desired withdraw amount, creates context state accounts (equality + range proofs), performs `confidential_transfer_withdraw`, then closes the proof accounts.

- `src/mint.rs`:

  - `TOKEN_DECIMALS: u8` — token decimal precision used by mint and operations.
  - `initialize_mint(rpc_client, payer)` — creates a new mint and initializes `ConfidentialTransferMint` extension. Returns the mint keypair and `Token` client.
  - `create_configure_ata(rpc_client, payer, mint_keypair)` — returns `(ata_pubkey, ElGamalKeypair, AeKey)` and handles:
    - Associated token account creation via `spl_associated_token_account::create_associated_token_account`.
    - Reallocation for `ConfidentialTransferAccount` extension using `spl_token_2022::instruction::reallocate`.
    - Client-side generation of ElGamal keypair and AES key derived from payer/ATA.
    - Construction of `configure_account` instruction(s) including pubkey validity proof data.
    - Sending the combined transaction and returning the configured ATA and local crypto material.

- `src/utils.rs`:
  - `load_keypair()` — loads the local Solana CLI keypair JSON from `$HOME/.config/solana/id.json` and returns a `Keypair`.

## Important crates / dependencies (from Cargo.toml)

- solana-client = 2.2.2 (nonblocking RpcClient used)
- solana-sdk = 2.2.2
- spl-associated-token-account = 6.0.0
- spl-token-client = 0.14.0 (Token client wrapper for Token-2022)
- spl-token-confidential-transfer-proof-extraction = 0.2.1
- spl-token-confidential-transfer-proof-generation = 0.3.0
- anyhow, dirs, serde_json, tokio

These crates implement the client-side logic for creating instructions, generating proofs, and interacting with the token program and the confidential transfer extensions.

## Build and run (local validator)

Assumptions:

- You have a local Solana validator listening on RPC `http://localhost:8899` (the example uses this URL). The repository expects the default Solana CLI keypair at `~/.config/solana/id.json`.

Typical quickstart (local dev):

```bash
# Start a local test validator in a separate terminal (if not already running):
solana-test-validator --reset

# Build and run the example (from repo root):
cargo run --release

# Or for faster debug cycles:
cargo run
```

The binary prints transaction signatures and progress for each step (mint creation, account configuration, mint_to, deposit, apply pending, proof account creation, withdraw, account close). RPC connection and payer keypair errors are common during initial setup — see Troubleshooting.

## Runtime configuration

- RPC URL is currently hard-coded in `src/main.rs` as `http://localhost:8899`. For other environments, change the `RpcClient::new_with_commitment(...)` call accordingly.

## Data shapes and key runtime types

- `ElGamalKeypair` (solana_zk_sdk) — used to create zero-knowledge proofs and decrypt ElGamal-encrypted values on the client.
- `AeKey` (auth_encryption) — AES key wrapper used to encrypt/decrypt balances and transfer amounts.
- `ConfidentialTransferAccount` extension — stored in token account extensions; holds confidential transfer metadata on-chain.
- `WithdrawProofData` — proof data produced client-side (includes equality and range proof data) required by on-chain withdraw verification.
- `ProofAccount::ContextAccount(pubkey)` — pointer to an on-chain account that holds serialized proof inputs or state for verification.

## Confidential transfer flow (detailed)

1. Configure ATA with `ConfidentialTransferAccount` extension. This allocates extra space on the token account to persist confidential transfer metadata.
2. Generate the ElGamal keypair and AES key client-side (deterministic/semi-deterministic from ATA+payer) and prepare an initial encrypted balance.
3. `mint_to` mints transparent tokens to the ATA. These are normal token balances.
4. `confidential_transfer_deposit` converts transparent tokens into the confidential pending balance (adds to pending balance counters and stores encrypted amounts).
5. `confidential_transfer_apply_pending_balance` decrypts and applies the pending balance to the account's available confidential balance. This step typically requires the ElGamal secret and AES key client-side.
6. Withdraw requires constructing zero-knowledge proofs proving knowledge of ciphertexts and ranges. The client uses `WithdrawAccountInfo::generate_proof_data(...)` and then uploads context state accounts containing the serialized proof inputs and calls `confidential_transfer_withdraw` with references to those context accounts.

Note: Proof account creation and verification may be split across transactions. This repository demonstrates creating context state accounts for equality and range proofs and then referencing them in the withdraw instruction.

## Security and operational notes

- Key management: The ElGamal secret and AES key are sensitive and used locally to generate/produce proofs. Do not commit or leak these secrets.
- Keyfile (`~/.config/solana/id.json`) must be protected. This repo reads it directly via `utils::load_keypair()`.
- Never use mainnet keys with this example without auditing and understanding the on-chain program IDs and proofs.
- Proof generation is performed client-side; ensure your runtime environment has enough memory/CPU for ZK proof generation.
- Rent considerations: proof context accounts are created and later closed to recover rent; ensure payer has sufficient lamports to fund temporary accounts.

## Testing and verification

- Local manual test: run against `solana-test-validator` and inspect accounts with `solana account <pubkey>` and `spl-token accounts` for token state.
- Verify ConfidentialTransferAccount extension presence by fetching account data via RPC and examining extensions via the Token client (the example uses `token.get_account_info(...).get_extension::<ConfidentialTransferAccount>()`).
- Add unit/integration tests by creating a small harness that spins up `solana-test-validator` (or uses `solana-program-test`) and runs the sequence programmatically.

## Troubleshooting

- Error: missing keypair file (`id.json`). Fix: create or point to a valid Solana keypair at `~/.config/solana/id.json`.
- RPC connection refused: ensure `solana-test-validator` is running and listening on `8899`, or change the RPC URL in `src/main.rs`.
- Transaction failures due to insufficient lamports: ensure the payer has enough SOL to create accounts and pay rent. Seed an account or airdrop in the local validator.
- Proof generation errors: check that the ElGamal/AES key generation succeeded and that the correct account extensions are present before attempting withdraw.

## Extending the example

- Add CLI flags to configure RPC URL, payer path, and behavior (mint amount, deposit amount, withdraw amount).
- Add tests using `solana-program-test` for deterministic unit tests that do not require `solana-test-validator` in a separate process.
- Add logging and structured error mappings for better observability.

## Developer notes

- Formatting: run `cargo fmt`.
- Linting: run `cargo clippy -- -D warnings` as needed.
- To change dependency versions, update `Cargo.toml` and run `cargo update`.

---

This document focuses on the technical details required to build, run, and reason about the confidential transfer example. For questions about production hardening, cryptography auditing, or integration with other token programs, open a dedicated design discussion and follow secure deployment practices.
