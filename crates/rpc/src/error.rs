//! Defines [RpcError], the Starknet JSON-RPC specification's error variants.
//!
//! In addition, it supplies the [generate_rpc_error_subset!] macro which should be used
//! by each JSON-RPC method to trivially create its subset of [RpcError] along with the boilerplate involved.
#![macro_use]

#[derive(serde::Serialize, Clone, Copy, Debug)]
pub enum TraceError {
    Received,
    Rejected,
}

/// The Starknet JSON-RPC error variants.
#[derive(thiserror::Error, Debug)]
pub enum RpcError {
    #[error("Failed to write transaction")]
    FailedToReceiveTxn,
    #[error("Contract not found")]
    ContractNotFound,
    #[error("Block not found")]
    BlockNotFound,
    #[error("Transaction hash not found")]
    TxnHashNotFoundV03,
    #[error("Invalid transaction index in a block")]
    InvalidTxnIndex,
    #[error("Invalid transaction hash")]
    InvalidTxnHash,
    #[error("Invalid block hash")]
    InvalidBlockHash,
    #[error("Class hash not found")]
    ClassHashNotFound,
    #[error("Transaction hash not found")]
    TxnHashNotFoundV04,
    #[error("Requested page size is too big")]
    PageSizeTooBig,
    #[error("There are no blocks")]
    NoBlocks,
    #[error("No trace available")]
    NoTraceAvailable(TraceError),
    #[error("The supplied continuation token is invalid or unknown")]
    InvalidContinuationToken,
    #[error("Too many keys provided in a filter")]
    TooManyKeysInFilter { limit: usize, requested: usize },
    #[error("Contract error")]
    ContractError,
    #[error("Invalid contract class")]
    InvalidContractClass,
    #[error("Class already declared")]
    ClassAlreadyDeclared,
    #[error("Invalid transaction nonce")]
    InvalidTransactionNonce,
    #[error("Max fee is smaller than the minimal transaction cost (validation plus fee transfer)")]
    InsufficientMaxFee,
    #[error("Account balance is smaller than the transaction's max_fee")]
    InsufficientAccountBalance,
    #[error("Account validation failed")]
    ValidationFailure,
    #[error("Compilation failed")]
    CompilationFailed,
    #[error("Contract class size it too large")]
    ContractClassSizeIsTooLarge,
    #[error("Sender address in not an account contract")]
    NonAccount,
    #[error("A transaction with the same hash already exists in the mempool")]
    DuplicateTransaction,
    #[error("The compiled class hash did not match the one supplied in the transaction")]
    CompiledClassHashMismatch,
    #[error("The transaction version is not supported")]
    UnsupportedTxVersion,
    #[error("The contract class version is not supported")]
    UnsupportedContractClassVersion,
    #[error("An unexpected error occurred")]
    UnexpectedError { data: String },
    #[error("Too many storage keys requested")]
    ProofLimitExceeded { limit: u32, requested: u32 },
    #[error(transparent)]
    GatewayError(starknet_gateway_types::error::StarknetError),
    #[error(transparent)]
    Internal(anyhow::Error),
}

impl RpcError {
    pub fn code(&self) -> i32 {
        match self {
            // Taken from the official starknet json rpc api.
            // https://github.com/starkware-libs/starknet-specs
            RpcError::FailedToReceiveTxn => 1,
            RpcError::NoTraceAvailable(_) => 10,
            RpcError::ContractNotFound => 20,
            RpcError::BlockNotFound => 24,
            RpcError::TxnHashNotFoundV03 => 25,
            RpcError::InvalidTxnHash => 25,
            RpcError::InvalidBlockHash => 26,
            RpcError::InvalidTxnIndex => 27,
            RpcError::ClassHashNotFound => 28,
            RpcError::TxnHashNotFoundV04 => 29,
            RpcError::PageSizeTooBig => 31,
            RpcError::NoBlocks => 32,
            RpcError::InvalidContinuationToken => 33,
            RpcError::TooManyKeysInFilter { .. } => 34,
            RpcError::ContractError => 40,
            RpcError::InvalidContractClass => 50,
            RpcError::ClassAlreadyDeclared => 51,
            RpcError::InvalidTransactionNonce => 52,
            RpcError::InsufficientMaxFee => 53,
            RpcError::InsufficientAccountBalance => 54,
            RpcError::ValidationFailure => 55,
            RpcError::CompilationFailed => 56,
            RpcError::ContractClassSizeIsTooLarge => 57,
            RpcError::NonAccount => 58,
            RpcError::DuplicateTransaction => 59,
            RpcError::CompiledClassHashMismatch => 60,
            RpcError::UnsupportedTxVersion => 61,
            RpcError::UnsupportedContractClassVersion => 62,
            RpcError::UnexpectedError { .. } => 63,
            // doc/rpc/pathfinder_rpc_api.json
            RpcError::ProofLimitExceeded { .. } => 10000,
            // https://www.jsonrpc.org/specification#error_object
            RpcError::GatewayError(_) | RpcError::Internal(_) => -32603,
        }
    }
}

/// Generates an enum subset of [RpcError] along with boilerplate for mapping the variants back to [RpcError].
///
/// This is useful for RPC methods which only emit a few of the [RpcError] variants as this macro can be
/// used to quickly create the enum-subset with the required glue code. This greatly improves the type safety
/// of the method.
///
/// ## Usage
/// ```ignore
/// generate_rpc_error_subset!(<enum_name>: <variant a>, <variant b>, <variant N>);
/// ```
/// Note that the variants __must__ match the [RpcError] variant names and that [RpcError::Internal]
/// is always included by default (and therefore should not be part of macro input).
///
/// An `Internal` only variant can be generated using `generate_rpc_error_subset!(<enum_name>)`.
///
/// ## Specifics
/// This macro generates the following:
///
/// 1. New enum definition with `#[derive(Debug)]`
/// 2. `impl From<NewEnum> for RpcError`
/// 3. `impl From<anyhow::Error> for NewEnum`
///
/// It always includes the `Internal(anyhow::Error)` variant.
///
/// ## Example with expansion
/// This macro invocation:
/// ```ignore
/// generate_rpc_error_subset!(MyEnum: BlockNotFound, NoBlocks);
/// ```
/// expands to:
/// ```ignore
/// #[derive(debug)]
/// pub enum MyError {
///     BlockNotFound,
///     NoBlocks,
///     Internal(anyhow::Error),
/// }
///
/// impl From<MyError> for RpcError {
///     fn from(x: MyError) -> Self {
///         match x {
///             MyError::BlockNotFound => Self::BlockNotFound,
///             MyError::NoBlocks => Self::NoBlocks,
///             MyError::Internal(internal) => Self::Internal(internal),
///         }
///     }
/// }
///
/// impl From<anyhow::Error> for MyError {
///     fn from(e: anyhow::Error) -> Self {
///         Self::Internal(e)
///     }
/// }
/// ```
#[allow(unused_macros)]
macro_rules! generate_rpc_error_subset {
    // This macro uses the following advanced techniques:
    //   - tt-muncher (https://danielkeep.github.io/tlborm/book/pat-incremental-tt-munchers.html)
    //   - push-down-accumulation (https://danielkeep.github.io/tlborm/book/pat-push-down-accumulation.html)
    //
    // All macro arms (except the entry-point) begin with `@XXX` to prevent accidental usage.
    //
    // It is possible to allow for custom `#[derive()]` and other attributes. These are currently not required
    // and therefore not implemented.

    // Entry-point for empty variant (with colon suffix)
    ($enum_name:ident:) => {
        generate_rpc_error_subset!($enum_name);
    };
    // Entry-point for empty variant (without colon suffix)
    ($enum_name:ident) => {
        generate_rpc_error_subset!(@enum_def, $enum_name,);
        generate_rpc_error_subset!(@from_anyhow, $enum_name);
        generate_rpc_error_subset!(@from_def, $enum_name,);
    };
    // Main entry-point for the macro
    ($enum_name:ident: $($subset:tt),+) => {
        generate_rpc_error_subset!(@enum_def, $enum_name, $($subset),+);
        generate_rpc_error_subset!(@from_anyhow, $enum_name);
        generate_rpc_error_subset!(@from_def, $enum_name, $($subset),+);
    };
    // Generates the enum definition, nothing tricky here.
    (@enum_def, $enum_name:ident, $($subset:tt),*) => {
        #[derive(Debug)]
        pub enum $enum_name {
            Internal(anyhow::Error),
            $($subset),*
        }
    };
    // Generates From<anyhow::Error>, nothing tricky here.
    (@from_anyhow, $enum_name:ident) => {
        impl From<anyhow::Error> for $enum_name {
            fn from(e: anyhow::Error) -> Self {
                Self::Internal(e)
            }
        }
    };
    // Generates From<$enum_name> for RpcError, this macro arm itself is not tricky,
    // however its child calls are.
    //
    // We pass down the variants, which will get tt-munched until we reach the base case.
    // We also initialize the match "arms" (`{}`) as empty. These will be accumulated until
    // we reach the base case at which point the match itself is generated using the arms.
    //
    // We cannot generate the match in this macro arm as each macro invocation must result in
    // a valid AST (I think). This means that having the match at this level would require the
    // child calls only return arm(s) -- which is not valid syntax by itself. Instead, the invalid
    // Rust is "pushed-down" as input to the child call -- instead of being the output (illegal).
    //
    // By pushing the arms from this level downwards, and creating the match statement at the lowest
    // level, we guarantee that only valid valid Rust will bubble back up.
    (@from_def, $enum_name:ident, $($variants:ident),*) => {
        impl From<$enum_name> for crate::error::RpcError {
            fn from(x: $enum_name) -> Self {
                generate_rpc_error_subset!(@parse, x, $enum_name, {}, $($variants),*)
            }
        }
    };
    // Termination case (no further input to munch). We generate the match statement here.
    (@parse, $var:ident, $enum_name:ident, {$($arms:tt)*}, $(,)*) => {
        match $var {
            $($arms)*
            $enum_name::Internal(internal) => Self::Internal(internal),
        }
    };
    // Special case for single variant. This could probably be folded into one of the other
    // cases but I struggled to do so correctly.
    (@parse, $var:ident, $enum_name:ident, {$($arms:tt)*}, $variant:ident) => {
        generate_rpc_error_subset!(
            @parse, $var, $enum_name,
            {
                $($arms)*
                $enum_name::$variant => Self::$variant,
            },
        )
    };
    // Append this variant to arms. Continue parsing the remaining variants.
    (@parse, $var:ident, $enum_name:ident, {$($arms:tt)*}, $variant:ident, $($tail:ident),*) => {
        generate_rpc_error_subset!(
            @parse, $var, $enum_name,
            {
                $($arms)*
                $enum_name::$variant => Self::$variant,
            },
            $($tail),*
        )
    };
}

#[allow(dead_code, unused_imports)]
pub(super) use generate_rpc_error_subset;

#[cfg(test)]
mod tests {
    mod rpc_error_subset {
        use super::super::{generate_rpc_error_subset, RpcError};
        use assert_matches::assert_matches;

        #[test]
        fn no_variant() {
            generate_rpc_error_subset!(Empty:);
            generate_rpc_error_subset!(EmptyNoColon);
        }

        #[test]
        fn single_variant() {
            generate_rpc_error_subset!(Single: ContractNotFound);

            let original = RpcError::from(Single::ContractNotFound);

            assert_matches!(original, RpcError::ContractNotFound);
        }

        #[test]
        fn multi_variant() {
            generate_rpc_error_subset!(Multi: ContractNotFound, NoBlocks, ContractError);

            let contract_not_found = RpcError::from(Multi::ContractNotFound);
            let no_blocks = RpcError::from(Multi::NoBlocks);
            let contract_error = RpcError::from(Multi::ContractError);

            assert_matches!(contract_not_found, RpcError::ContractNotFound);
            assert_matches!(no_blocks, RpcError::NoBlocks);
            assert_matches!(contract_error, RpcError::ContractError);
        }
    }
}
