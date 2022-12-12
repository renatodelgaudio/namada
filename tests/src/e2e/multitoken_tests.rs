use color_eyre::eyre::Result;

use super::helpers::get_actor_rpc;
use super::setup::constants::{
    wasm_abs_path, BERTHA, NAM, VP_ALWAYS_TRUE_WASM,
};
use super::setup::{self, Bin, Who};
use crate::{run, run_as};

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
        BERTHA, // arbitrarily BERTHA
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
    let mut client = run!(test, Bin::Client, init_account_args, Some(40))?;
    client.exp_string("Transaction is valid.")?;
    client.assert_success();

    // establish a multitoken VP
    // - #atest5blah/tokens/red/balance/$albert = 100
    // - #atest5blah/tokens/red/balance/$bertha = 0

    // make a transfer from Albert to Bertha, signed by Christel - this should
    // be rejected
    Ok(())
}
