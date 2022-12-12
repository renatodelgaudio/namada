use color_eyre::eyre::Result;
use namada_core::types::address::Address;
use namada_core::types::storage;
use namada_tx_prelude::storage::KeySeg;

use super::helpers::get_actor_rpc;
use super::setup::constants::{
    wasm_abs_path, BERTHA, NAM, VP_ALWAYS_TRUE_WASM,
};
use super::setup::{self, Bin, Test, Who};
use crate::e2e::setup::constants::{ALBERT, TX_WRITE_STORAGE_KEY_WASM};
use crate::{run, run_as};

const MULTITOKEN_KEY_SEGMENT: &str = "tokens";
const BALANCE_KEY_SEGMENT: &str = "balance";

const ARBITRARY_SIGNER: &str = ALBERT;

fn get_address_for_alias(test: &Test, alias: &str) -> Result<Address> {
    let wallet_find_address = run!(
        test,
        Bin::Wallet,
        vec!["address", "find", "--alias", alias],
        Some(40)
    )?
    .background();
    // sleep a couple secs so the command has chance to run? maybe this isn't
    // necessary
    std::thread::sleep(std::time::Duration::from_secs(2));
    let mut wallet_find_address = wallet_find_address.foreground();

    let (_, addr) = wallet_find_address.exp_regex(r"atest1\w+")?;
    Ok(Address::decode(addr)?)
}
#[test]
fn test_multitoken_transfer_implicit_to_implicit() -> Result<()> {
    let test = setup::single_node_net()?;
    let mut ledger =
        run_as!(test, Who::Validator(0), Bin::Node, &["ledger"], Some(40))?;
    ledger.exp_string("Namada ledger node started")?;
    // TODO(namada#867): we only need to wait until the RPC server is available,
    // not necessarily for a block to be committed
    // ledger.exp_string("Starting RPC HTTP server on")?;
    ledger.exp_regex(r"Committed block hash.*, height: [0-9]+")?;
    let _bg_ledger = ledger.background();

    let rpc_addr = get_actor_rpc(&test, &Who::Validator(0));

    // we use a VP that always returns true for the multitoken VP here, as we
    // are testing out the VPs of the sender and receiver of multitoken
    // transactions here - not any multitoken VP itself
    let multitoken_vp_wasm_path = wasm_abs_path(VP_ALWAYS_TRUE_WASM)
        .to_string_lossy()
        .to_string();
    let multitoken_alias = "multitoken";

    let init_account_args = vec![
        "init-account",
        "--source",
        ARBITRARY_SIGNER,
        "--public-key",
        // Value obtained from
        // `namada::types::key::ed25519::tests::gen_keypair`
        "001be519a321e29020fa3cbfbfd01bd5e92db134305609270b71dace25b5a21168",
        "--code-path",
        &multitoken_vp_wasm_path,
        "--alias",
        multitoken_alias,
        "--gas-amount",
        "0",
        "--gas-limit",
        "0",
        "--gas-token",
        NAM,
        "--ledger-address",
        &rpc_addr,
    ];
    let mut client_init_account =
        run!(test, Bin::Client, init_account_args, Some(40))?;
    client_init_account.exp_string("Transaction is valid.")?;
    client_init_account.exp_string("Transaction applied")?;
    client_init_account.assert_success();

    // establish a multitoken VP with the following balances
    // - #atest5blah/tokens/red/balance/$albert = 100
    // - #atest5blah/tokens/red/balance/$bertha = 0

    let multitoken_vp_addr = get_address_for_alias(&test, multitoken_alias)?;
    println!("Fake multitoken VP established at {}", multitoken_vp_addr);

    const RED_TOKEN_KEY_SEGMENT: &str = "red";

    let albert_addr = get_address_for_alias(&test, ALBERT)?;
    let albert_red_balance = storage::Key::from(multitoken_vp_addr.to_db_key())
        .push(&MULTITOKEN_KEY_SEGMENT.to_owned())?
        .push(&RED_TOKEN_KEY_SEGMENT.to_owned())?
        .push(&BALANCE_KEY_SEGMENT.to_owned())?
        .push(&albert_addr)?;
    println!("Albert's red token balance key: {}", albert_red_balance);

    let bertha_addr = get_address_for_alias(&test, BERTHA)?;
    let bertha_red_balance = storage::Key::from(multitoken_vp_addr.to_db_key())
        .push(&MULTITOKEN_KEY_SEGMENT.to_owned())?
        .push(&RED_TOKEN_KEY_SEGMENT.to_owned())?
        .push(&BALANCE_KEY_SEGMENT.to_owned())?
        .push(&bertha_addr)?;
    println!("Bertha's red token balance key: {}", bertha_red_balance);

    let tx_code_path = wasm_abs_path(TX_WRITE_STORAGE_KEY_WASM);
    let tx_data_path = test
        .test_dir
        .path()
        .join("albert_red_token_balance_key.txt");
    std::fs::write(&tx_data_path, format!("{albert_red_balance}"))?;

    let tx_data_path = tx_data_path.to_string_lossy().to_string();
    let tx_code_path = tx_code_path.to_string_lossy().to_string();
    let tx_args = vec![
        "tx",
        "--signer",
        ARBITRARY_SIGNER,
        "--code-path",
        &tx_code_path,
        "--data-path",
        &tx_data_path,
        "--ledger-address",
        &rpc_addr,
    ];
    let mut client_tx = run!(test, Bin::Client, tx_args, Some(40))?;
    client_tx.exp_string("Transaction is valid.")?;
    client_tx.exp_string("Transaction applied")?;
    client_tx.assert_success();

    // make a transfer from Albert to Bertha, signed by Christel - this should
    // be rejected
    Ok(())
}
