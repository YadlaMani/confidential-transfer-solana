use anyhow::Result;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
   
    pubkey::Pubkey, signature::Keypair, signer::Signer, transaction::Transaction
};
use spl_associated_token_account::{
    get_associated_token_address_with_program_id, instruction::create_associated_token_account,
};
use spl_token_client::{
    client::{ProgramRpcClient, ProgramRpcClientSendTransaction},
    spl_token_2022::{
        extension::{
            ExtensionType,
            confidential_transfer::instruction::{PubkeyValidityProofData, configure_account},
        },
        id as token_2022_program_id,
        instruction::reallocate,
        solana_zk_sdk::encryption::{auth_encryption::AeKey, elgamal::ElGamalKeypair},
    },
    token::{ExtensionInitializationParams, Token},
};
use spl_token_confidential_transfer_proof_extraction::instruction::{ProofData, ProofLocation};
use std::sync::Arc;

pub const TOKEN_DECIMALS: u8 = 9;
//The maximum number of Deposit or Transfer instructions that can credit (add) to the 
//pending_balance before the recipient must issue an ApplyPendingBalance instruction.
const MAXIMUM_PENDING_BALANCE_COUNTER: u64 = 128;

// Function to initialize a new token mint with ConfidentialTransferMint extension
pub async fn initialize_mint(
    rpc_client: Arc<RpcClient>,
    payer: Arc<dyn Signer>,
) -> Result<(Keypair, Token<ProgramRpcClientSendTransaction>)> {
    let mint_keypair=Keypair::new();
  
    let program_client=ProgramRpcClient::new(rpc_client.clone(),ProgramRpcClientSendTransaction);
    let token=Token::new(
        Arc::new(program_client),
        &token_2022_program_id(),
        &mint_keypair.pubkey(),
        Some(TOKEN_DECIMALS),
        payer.clone()
    );
    //ConfidentialTransferMint extension enables confidential (private) transfers of tokens
    let extension_init_params=vec![
        ExtensionInitializationParams::ConfidentialTransferMint { 
            authority: Some(payer.pubkey()), //Authority to manage confidential transfer settings
            auto_approve_new_accounts: true, //Automatically approve new confidential transfer accounts
            auditor_elgamal_pubkey: None //No auditor 
        }
    ];
   
    let transaction_sig=token
    .create_mint(
        &payer.pubkey(),
        Some(&payer.pubkey()),
        extension_init_params,
        &[&mint_keypair],
    ).await?;
    println!("Mint creation transaction signature: {}", transaction_sig);
   
     Ok((mint_keypair, token))   
}

// Function to create and configure an associated token account (ATA) for confidential transfers
pub async fn create_configure_ata(
    rpc_client: Arc<RpcClient>,
    payer: Arc<dyn Signer>,
    mint_keypair: &Keypair,
) -> Result<(Pubkey,ElGamalKeypair,AeKey)> {
     //Configure token account for confidential transfers
    let ata_pubkey=get_associated_token_address_with_program_id(
        &payer.pubkey(),//Owner of the token account
        &mint_keypair.pubkey(),//Token mint
        &token_2022_program_id(),//Token program ID
    );
    //Step1:Creating associated token account 
    let created_ata_ix=create_associated_token_account(
        &payer.pubkey(),//Payer for the creation of token account
        &payer.pubkey(),//Owner of the token account
        &mint_keypair.pubkey(),//Token mint
        &token_2022_program_id(),//Token program ID
    );
    //Step2:Reallocate the token account to include space for ConfidentialTransferAccount extension
    let reallocate_ix=reallocate(
        &token_2022_program_id(),//Token program ID
        &ata_pubkey,//ATA public key
        &payer.pubkey(),//Payer
        &payer.pubkey(),//Token account owner
        &[&payer.pubkey()],//Signers
        &[ExtensionType::ConfidentialTransferAccount]//Extensions to add
    )?;
    //Step3:Generate ElGamal keypair and AES key for token account
    //Elgamal keypair is used to generate zero-knowledge proofs for confidential transfers
    //AES key is used to encrypt and decrypt confidential balances
    let elgamal_keypair=ElGamalKeypair::new_from_signer(&payer,&ata_pubkey.to_bytes()).expect("Failed to generate ElGamal keypair");
    let aes_keypair=AeKey::new_from_signer(&payer, &ata_pubkey.to_bytes()).expect("Failed to generate AES key");
    //Initial balance
    let decryptable_balance=aes_keypair.encrypt(0);
    //Generate the proof data client side
    let proof_data=PubkeyValidityProofData::new(&elgamal_keypair).map_err(|_|anyhow::anyhow!("Failed to generate pubkey validity proof data"))?;
    let proof_location=ProofLocation::InstructionOffset(1.try_into()?,ProofData::InstructionData(&proof_data));
    //Step4:Configure account for confidential transfers
    let configure_account_ix=configure_account(
        &token_2022_program_id(), //Program Id
        &ata_pubkey, //Token account
        &mint_keypair.pubkey(), //Mint account
        &decryptable_balance.into(), //Initial balance
        MAXIMUM_PENDING_BALANCE_COUNTER,
        &payer.pubkey(),//Token account owner
        &[],//Additional signers
        proof_location //Proof location
    )?;
    let mut ixs=vec![
        created_ata_ix,
        reallocate_ix,
       
    ];
    ixs.extend(configure_account_ix);
    let recent_blockhash=rpc_client.get_latest_blockhash().await?;
    let transaction=Transaction::new_signed_with_payer(
        &ixs,
        Some(&payer.pubkey()),
        &[&payer],
        recent_blockhash,
    );
    let transaction_sig=rpc_client.send_and_confirm_transaction(&transaction).await?;
    println!("Confidential transfer account configuration transaction signature: {}", transaction_sig);
    
    Ok((ata_pubkey,elgamal_keypair,aes_keypair))
}
