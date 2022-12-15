//! Tests for multitoken functionality
use color_eyre::eyre::Result;
use namada_core::types::token;

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
    // - #atest5blah/tokens/red/balance/$albert_established = 100
    // - #atest5blah/tokens/red/balance/$bertha = 0

    let multitoken_vp_addr =
        e2e::helpers::find_address(&test, &multitoken_alias)?;
    println!("Fake multitoken VP established at {}", multitoken_vp_addr);

    let albert_addr = e2e::helpers::find_address(&test, ALBERT)?;
    helpers::mint_red_tokens(
        &test,
        &rpc_addr,
        &multitoken_vp_addr,
        &albert_addr,
        &token::Amount::from(100_000_000),
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
        &token::Amount::from(10_000_000),
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
        &token::Amount::from(10_000_000),
    )?;
    authorized_transfer.exp_string("Transaction applied with result")?;
    authorized_transfer.exp_string("Transaction is valid")?;
    authorized_transfer.assert_success();
    Ok(())
}

#[test]
fn test_multitoken_transfer_established_to_implicit() -> Result<()> {
    let (test, _ledger) = e2e::helpers::setup_single_node_test()?;

    let rpc_addr = get_actor_rpc(&test, &Who::Validator(0));
    let multitoken_alias = helpers::init_multitoken_vp(&test, &rpc_addr)?;

    let multitoken_vp_addr =
        e2e::helpers::find_address(&test, &multitoken_alias)?;
    println!("Fake multitoken VP established at {}", multitoken_vp_addr);

    // create a new implicit account which will control the sender established
    // account
    let controlling_alias = "controlling";
    e2e::helpers::new_implicit_account(&test, controlling_alias)?;

    // create a new implicit account which *doesn't* control the sender
    // established account
    let noncontrolling_alias = "noncontrolling";
    e2e::helpers::new_implicit_account(&test, noncontrolling_alias)?;

    // create an established account that we control
    let established_alias = "established";
    helpers::init_established_account(
        &test,
        &rpc_addr,
        controlling_alias,
        established_alias,
    )?;

    // mint some red tokens for the established account
    let established_addr =
        e2e::helpers::find_address(&test, established_alias)?;
    helpers::mint_red_tokens(
        &test,
        &rpc_addr,
        &multitoken_vp_addr,
        &established_addr,
        &token::Amount::from(100_000_000),
    )?;

    // attempt an unauthorized transfer to Albert from the established account
    let mut unauthorized_transfer = helpers::attempt_red_tokens_transfer(
        &test,
        &rpc_addr,
        &multitoken_alias,
        established_alias,
        ALBERT,
        noncontrolling_alias,
        &token::Amount::from(10_000_000),
    )?;
    unauthorized_transfer.exp_string("Transaction applied with result")?;
    unauthorized_transfer.exp_string("Transaction is invalid")?;
    unauthorized_transfer
        .exp_string(&format!("Rejected: {established_addr}"))?;
    unauthorized_transfer.assert_success();

    // attempt an authorized transfer to Albert from the established account
    let mut authorized_transfer = helpers::attempt_red_tokens_transfer(
        &test,
        &rpc_addr,
        &multitoken_alias,
        established_alias,
        ALBERT,
        established_alias,
        &token::Amount::from(10_000_000),
    )?;
    authorized_transfer.exp_string("Transaction applied with result")?;
    authorized_transfer.exp_string("Transaction is valid")?;
    authorized_transfer.assert_success();

    Ok(())
}
