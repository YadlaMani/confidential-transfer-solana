
use std::sync::Arc;

use anyhow::{Context, Result};
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};
use dirs;
use spl_token_client::{
    
    client::{ProgramRpcClientSendTransaction, RpcClientResponse}, token::Token
};
pub fn load_keypair()->Result<Keypair>{
    // Load the keypair from the default Solana CLI location
    let keypair_path=dirs::home_dir().context("Unable to get home directory")?.join(".config/solana/id.json");
    // Read the keypair file
    let file=std::fs::File::open(&keypair_path)?;
    let keypair_bytes:Vec<u8>=serde_json::from_reader(file)?;
    let keypair=Keypair::try_from(&keypair_bytes[..])?;
    Ok(keypair)
}
