//! Tests for multitoken functionality
use color_eyre::eyre::Result;

use super::helpers::get_actor_rpc;
use super::setup::constants::{ALBERT, BERTHA, CHRISTEL};
use super::setup::{self, Who};
use crate::e2e;

mod helpers;

#[test]
fn test_multitoken_transfer_implicit_to_implicit() -> Result<()> {
    let (test, _ledger) = e2e::helpers::setup_single_node_test()?;

    let rpc_addr = get_actor_rpc(&test, &Who::Validator(0));
    let multitoken_alias = helpers::init_multitoken_vp(&test, &rpc_addr)?;

    // establish a multitoken VP with the following balances
    // - #atest5blah/tokens/red/balance/$albert = 100
    // - #atest5blah/tokens/red/balance/$bertha = 0

    let multitoken_vp_addr =
        helpers::get_address_for_alias(&test, &multitoken_alias)?;
    println!("Fake multitoken VP established at {}", multitoken_vp_addr);

    let albert_addr = helpers::get_address_for_alias(&test, ALBERT)?;
    helpers::mint_red_tokens(
        &test,
        &rpc_addr,
        &multitoken_vp_addr,
        &albert_addr,
    )?;

    // make a transfer from Albert to Bertha, signed by Christel - this should
    // be rejected
    let mut unauthorized_transfer = helpers::attempt_red_tokens_transfer(
        &test,
        &rpc_addr,
        &multitoken_alias,
        ALBERT,
        BERTHA,
        CHRISTEL,
    )?;
    unauthorized_transfer.exp_string("Transaction applied with result")?;
    unauthorized_transfer.exp_string("Transaction is invalid")?;
    unauthorized_transfer.exp_string(&format!("Rejected: {albert_addr}"))?;
    unauthorized_transfer.assert_success();

    // make a transfer from Albert to Bertha, signed by Albert - this should
    // be accepted
    let mut authorized_transfer = helpers::attempt_red_tokens_transfer(
        &test,
        &rpc_addr,
        &multitoken_alias,
        ALBERT,
        BERTHA,
        ALBERT,
    )?;
    authorized_transfer.exp_string("Transaction applied with result")?;
    authorized_transfer.exp_string("Transaction is valid")?;
    authorized_transfer.assert_success();
    Ok(())
}
