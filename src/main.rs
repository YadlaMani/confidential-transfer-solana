use anyhow::Result;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::Keypair,
    signer::Signer,
   
};

use spl_token_client::{
    client::ProgramRpcClientSendTransaction, spl_token_2022::{extension::{BaseStateWithExtensions, confidential_transfer::{ConfidentialTransferAccount, account_info::WithdrawAccountInfo}}, solana_zk_sdk::encryption::elgamal}, token::Token
};
use spl_token_confidential_transfer_proof_generation::withdraw::WithdrawProofData;

use std::sync::Arc;

mod mint;
mod utils;


#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the RPC client to connect to the local Solana cluster
    let rpc_client = Arc::new(RpcClient::new_with_commitment(
        String::from("http://localhost:8899"),
        CommitmentConfig::confirmed(),
    ));

    // Load payer keypair
    let payer = Arc::new(utils::load_keypair()?);
    println!("Payer public key: {}", payer.pubkey());

    // Token Mint Account creation and initialization
    let (mint_keypair, token): (Keypair, Token<ProgramRpcClientSendTransaction>) =
        mint::initialize_mint(rpc_client.clone(), payer.clone()).await?;
    println!("Mint Account public key: {}", mint_keypair.pubkey());

    // Configure token account for confidential transfers
    // ElGamal keypair for public-key cryptography (decryption and ZK proofs)
    // AES key for encryption of balance and transfer amounts
    let (ata_pubkey,elgamal_keypair,aeskey) =
        mint::create_configure_ata(rpc_client.clone(), payer.clone(), &mint_keypair).await?;
    println!(
        "Associated token account configured for confidential transfers: {}",
        ata_pubkey
    );
    //Mint tokens to the newly crated ata
    let mint_sig=token.mint_to(
        &ata_pubkey,//destination ata
        &payer.pubkey(),//mint authority
        100*10u64.pow(mint::TOKEN_DECIMALS as u32),//amount to mint
        &[&payer]//signers
    ).await?;
    println!("Minted tokens transaction signature: {}", mint_sig);
    //Deposit token to confidential state
    //Converts normal tokens -> confidential tokens
    let deposit_sig=token.confidential_transfer_deposit(
        &ata_pubkey,//deestination ata
        &payer.pubkey(),//authority(owner) of the account
        50*10u64.pow(mint::TOKEN_DECIMALS as u32),//amount to deposit
        mint::TOKEN_DECIMALS,//decimals
        &[&payer]//signer(owner of the ata)
    ).await?;
    println!("Confidential transfer deposit transaction signature: {}", deposit_sig);
    //Appy pending balance to make the funds available for confidential transfers
    let apply_signature=token.confidential_transfer_apply_pending_balance(
        &ata_pubkey,//ata public key
        &payer.pubkey(),//owner of the ata
        None,//Optional new decryptable available balance
        elgamal_keypair.secret(),
        &aeskey,
        &[&payer],//Signers(owner must sign)
    ).await?;
    println!("Apply pending balance transaction signature: {}", apply_signature);
    println!("Confidential transfer setup complete.Tokens are now available for confidential transfers.");
    //Withdraw tokens from confidential state back to normal tokens
    let withdraw_amount=20*10u64.pow(mint::TOKEN_DECIMALS as u32);
    let token_account=token.get_account_info(&mint_keypair.pubkey()).await?;
    let extension_data=token_account.get_extension::<ConfidentialTransferAccount>()?;
    //Confidential transfer extension information needed to construct a withdraw instruction 
    let withdraw_account=WithdrawAccountInfo::new(
        extension_data,
    );
    //create keypairs for the proof accounts
    let equality_proof_context_state_keypair=Keypair::new();
    let equality_proof_context_state_pubkey=equality_proof_context_state_keypair.pubkey();
    let range_proof_context_state_keypair=Keypair::new();
    let range_proof_context_state_pubkey=range_proof_context_state_keypair.pubkey();
    //Withdraw proof data
    let WithdrawProofData{
        equality_proof_data,
        range_proof_data,
    }=withdraw_account.generate_proof_data(withdraw_amount, &elgamal_keypair, &aeskey)?;
    //Generate equality proof account
    let equality_proof_sig=token.confidential_transfer_create_context_state_account(
        &equality_proof_context_state_pubkey,//Public key for the equality proof account
        &payer.pubkey(),//Authority that can manage the account
        &equality_proof_data,//Proof data for the equality proof
        false,//Fals:combine account creation+proof verification in one transaction
        &[&payer,&equality_proof_context_state_keypair],//signer of the new account
    ).await?;
    println!("Equality proof account creation transaction signature: {}", equality_proof_sig);
    //Generate range proof account
    let range_proof_sig=token.confidential_transfer_create_context_state_account(
        &range_proof_context_state_pubkey,//Public key for the range proof account
        &payer.pubkey(),//Authority that can manage the account
        &range_proof_data,//Proof data for the range proof
        false,//Fals:combine account creation+proof verification in one transaction
        &[&payer,&range_proof_context_state_keypair],//signer of the new account
    ).await?;
    println!("Range proof account creation transaction signature: {}", range_proof_sig);
    println!("Performing withdrawl from confidential state back to normal tokens...");
    //Perform the withdraw from confidential state back to normal tokens
    let withdraw_sig=token.confidential_transfer_withdraw(
        &ata_pubkey,//Source ata
        &payer.pubkey(),//Owner of the ata
       Some(&spl_token_client::token::ProofAccount::ContextAccount(
        equality_proof_context_state_pubkey//Reference to equality proof account
       )),
         Some(&spl_token_client::token::ProofAccount::ContextAccount(
        range_proof_context_state_pubkey//Reference to range proof account
         )),
         withdraw_amount,//Amount to withdraw
        mint::TOKEN_DECIMALS,//decimals
        Some(withdraw_account),
        &elgamal_keypair,
        &aeskey,
        &[&payer],
    ).await?;
    println!("Confidential transfer withdraw transaction signature: {}", withdraw_sig);
    //Close the context state accounts to recover rent
    println!("Closing proof context state accounts to recover rent...");
    let close_equality_sig=token.confidential_transfer_close_context_state_account(
        &equality_proof_context_state_pubkey,//Public key of the equality proof account
        &payer.pubkey(),//Authority that can close the account
        &payer.pubkey(),//Destination to receive recovered rent
        &[&payer],//Signer(authority)

    ).await?;
    println!("Close equality proof account transaction signature: {}", close_equality_sig);
    let close_range_sig=token.confidential_transfer_close_context_state_account(
        &range_proof_context_state_pubkey,//Public key of the range proof account
        &payer.pubkey(),//Authority that can close the account
        &payer.pubkey(),//Destination to receive recovered rent
        &[&payer],//Signer(authority)  
    ).await?;
    println!("Close range proof account transaction signature: {}", close_range_sig);
    Ok(())
}