use color_eyre::eyre::Result;

#[test]
fn test_multitoken_transfer_implicit_to_implicit() -> Result<()> {
    // establish a multitoken VP
    // - #atest5blah/tokens/red/balance/$albert = 100
    // - #atest5blah/tokens/red/balance/$bertha = 0

    // make a transfer from Albert to Bertha, signed by Christel - this should
    // be rejected
    Ok(())
}
