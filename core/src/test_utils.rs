//! Utilities for use in tests.

use std::path::PathBuf;

/// Corresponds to wasms that we build for tests (under the `wasm_for_tests/`
/// directory).
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy)]
pub enum TestWasms {
    TxNoOp,
    TxMemoryLimit,
    TxInitProposal,
    TxMintTokens,
    TxProposalCode,
    VpAlwaysTrue,
    VpEval,
    VpMemoryLimit,
}

impl TestWasms {
    /// Get the path to this test wasm.
    pub fn path(&self) -> PathBuf {
        let filename = match self {
            TestWasms::TxNoOp => "tx_no_op.wasm",
            TestWasms::TxMemoryLimit => "tx_memory_limit.wasm",
            TestWasms::TxInitProposal => "tx_init_proposal.wasm",
            TestWasms::TxMintTokens => "tx_mint_tokens.wasm",
            TestWasms::TxProposalCode => "tx_proposal_code.wasm",
            TestWasms::VpAlwaysTrue => "vp_always_true.wasm",
            TestWasms::VpEval => "vp_eval.wasm",
            TestWasms::VpMemoryLimit => "vp_memory_limit.wasm",
        };
        PathBuf::from("../wasm_for_tests").join(filename)
    }

    /// Attempts to read the contents of this test wasm. Panics if it is not
    /// able to for any reason.
    pub fn bytes(&self) -> Vec<u8> {
        let path = self.path();
        std::fs::read(&path).unwrap_or_else(|_| {
            panic!("Could not read wasm at path {}", path.to_string_lossy())
        })
    }
}
