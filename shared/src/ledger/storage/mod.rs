//! Ledger's state storage with key-value backed store and a merkle tree

mod ics23_specs;
mod merkle_tree;
#[cfg(any(test, feature = "testing"))]
pub mod mockdb;
pub mod traits;
pub mod types;
pub mod write_log;

use core::fmt::Debug;
use std::array;
use std::collections::btree_set;

use borsh::{BorshDeserialize, BorshSerialize};
use ferveo_common::TendermintValidator;
use thiserror::Error;

use super::parameters::Parameters;
use super::storage_api::{ResultExt, StorageRead, StorageWrite};
use super::{parameters, storage_api};
use crate::ledger::gas::MIN_STORAGE_GAS;
use crate::ledger::parameters::EpochDuration;
use crate::ledger::pos::namada_proof_of_stake::types::VotingPower;
use crate::ledger::pos::namada_proof_of_stake::PosBase;
use crate::ledger::pos::types::WeightedValidator;
use crate::ledger::pos::{PosParams, ValidatorSets};
use crate::ledger::storage::merkle_tree::{
    Error as MerkleTreeError, MerkleRoot,
};
pub use crate::ledger::storage::merkle_tree::{
    MerkleTree, MerkleTreeStoresRead, MerkleTreeStoresWrite, StoreRef,
    StoreType,
};
pub use crate::ledger::storage::traits::StorageHasher;
use crate::ledger::storage_api::queries::{self, QueriesExt, SendValsetUpd};
use crate::tendermint::merkle::proof::Proof;
use crate::tendermint_proto::google::protobuf;
use crate::tendermint_proto::types::EvidenceParams;
use crate::types::address::{Address, EstablishedAddressGen, InternalAddress};
use crate::types::chain::{ChainId, CHAIN_ID_LENGTH};
use crate::types::ethereum_events::EthAddress;
use crate::types::key;
use crate::types::key::dkg_session_keys::DkgPublicKey;
#[cfg(feature = "ferveo-tpke")]
use crate::types::storage::TxQueue;
use crate::types::storage::{
    BlockHash, BlockHeight, Epoch, Epochs, Header, Key, KeySeg,
    MembershipProof, MerkleValue, BLOCK_HASH_LENGTH,
};
use crate::types::time::DateTimeUtc;
use crate::types::token::{self, Amount};
use crate::types::transaction::EllipticCurve;
use crate::types::vote_extensions::validator_set_update::EthAddrBook;

/// A result of a function that may fail
pub type Result<T> = std::result::Result<T, Error>;
/// The maximum size of an IBC key (in bytes) allowed in merkle-ized storage
pub const IBC_KEY_LIMIT: usize = 120;

/// The storage data
#[derive(Debug)]
pub struct Storage<D, H>
where
    D: DB + for<'iter> DBIter<'iter>,
    H: StorageHasher,
{
    /// The database for the storage
    pub db: D,
    /// The ID of the chain
    pub chain_id: ChainId,
    /// The storage for the current (yet to be committed) block
    pub block: BlockStorage<H>,
    /// The latest block header
    pub header: Option<Header>,
    /// The height of the committed block
    pub last_height: BlockHeight,
    /// The epoch of the committed block
    pub last_epoch: Epoch,
    /// Minimum block height at which the next epoch may start
    pub next_epoch_min_start_height: BlockHeight,
    /// Minimum block time at which the next epoch may start
    pub next_epoch_min_start_time: DateTimeUtc,
    /// The current established address generator
    pub address_gen: EstablishedAddressGen,
    /// Wrapper txs to be decrypted in the next block proposal
    #[cfg(feature = "ferveo-tpke")]
    pub tx_queue: TxQueue,
}

/// The block storage data
#[derive(Debug)]
pub struct BlockStorage<H: StorageHasher> {
    /// Merkle tree of all the other data in block storage
    pub tree: MerkleTree<H>,
    /// Hash of the block
    pub hash: BlockHash,
    /// Height of the block (i.e. the level)
    pub height: BlockHeight,
    /// Epoch of the block
    pub epoch: Epoch,
    /// Predecessor block epochs
    pub pred_epochs: Epochs,
}

#[allow(missing_docs)]
#[derive(Error, Debug)]
pub enum Error {
    #[error("TEMPORARY error: {error}")]
    Temporary { error: String },
    #[error("Found an unknown key: {key}")]
    UnknownKey { key: String },
    #[error("Storage key error {0}")]
    KeyError(crate::types::storage::Error),
    #[error("Coding error: {0}")]
    CodingError(types::Error),
    #[error("Merkle tree error: {0}")]
    MerkleTreeError(MerkleTreeError),
    #[error("DB error: {0}")]
    DBError(String),
    #[error("Borsh (de)-serialization error: {0}")]
    BorshCodingError(std::io::Error),
    #[error("Merkle tree at the height {height} is not stored")]
    NoMerkleTree { height: BlockHeight },
}

/// The block's state as stored in the database.
pub struct BlockStateRead {
    /// Merkle tree stores
    pub merkle_tree_stores: MerkleTreeStoresRead,
    /// Hash of the block
    pub hash: BlockHash,
    /// Height of the block
    pub height: BlockHeight,
    /// Epoch of the block
    pub epoch: Epoch,
    /// Predecessor block epochs
    pub pred_epochs: Epochs,
    /// Minimum block height at which the next epoch may start
    pub next_epoch_min_start_height: BlockHeight,
    /// Minimum block time at which the next epoch may start
    pub next_epoch_min_start_time: DateTimeUtc,
    /// Established address generator
    pub address_gen: EstablishedAddressGen,
    /// Wrapper txs to be decrypted in the next block proposal
    #[cfg(feature = "ferveo-tpke")]
    pub tx_queue: TxQueue,
}

/// The block's state to write into the database.
pub struct BlockStateWrite<'a> {
    /// Merkle tree stores
    pub merkle_tree_stores: MerkleTreeStoresWrite<'a>,
    /// Header of the block
    pub header: Option<&'a Header>,
    /// Hash of the block
    pub hash: &'a BlockHash,
    /// Height of the block
    pub height: BlockHeight,
    /// Epoch of the block
    pub epoch: Epoch,
    /// Predecessor block epochs
    pub pred_epochs: &'a Epochs,
    /// Minimum block height at which the next epoch may start
    pub next_epoch_min_start_height: BlockHeight,
    /// Minimum block time at which the next epoch may start
    pub next_epoch_min_start_time: DateTimeUtc,
    /// Established address generator
    pub address_gen: &'a EstablishedAddressGen,
    /// Wrapper txs to be decrypted in the next block proposal
    #[cfg(feature = "ferveo-tpke")]
    pub tx_queue: &'a TxQueue,
}

/// A database backend.
pub trait DB: std::fmt::Debug {
    /// A DB's cache
    type Cache;
    /// A handle for batch writes
    type WriteBatch: DBWriteBatch;

    /// Open the database from provided path
    fn open(
        db_path: impl AsRef<std::path::Path>,
        cache: Option<&Self::Cache>,
    ) -> Self;

    /// Flush data on the memory to persistent them
    fn flush(&self, wait: bool) -> Result<()>;

    /// Read the last committed block's metadata
    fn read_last_block(&mut self) -> Result<Option<BlockStateRead>>;

    /// Write block's metadata
    fn write_block(&mut self, state: BlockStateWrite) -> Result<()>;

    /// Read the block header with the given height from the DB
    fn read_block_header(&self, height: BlockHeight) -> Result<Option<Header>>;

    /// Read the merkle tree stores with the given height
    fn read_merkle_tree_stores(
        &self,
        height: BlockHeight,
    ) -> Result<Option<MerkleTreeStoresRead>>;

    /// Read the latest value for account subspace key from the DB
    fn read_subspace_val(&self, key: &Key) -> Result<Option<Vec<u8>>>;

    /// Read the value for account subspace key at the given height from the DB.
    /// In our `PersistentStorage` (rocksdb), to find a value from arbitrary
    /// height requires looking for diffs from the given `height`, possibly
    /// up to the `last_height`.
    fn read_subspace_val_with_height(
        &self,
        key: &Key,
        height: BlockHeight,
        last_height: BlockHeight,
    ) -> Result<Option<Vec<u8>>>;

    /// Write the value with the given height and account subspace key to the
    /// DB. Returns the size difference from previous value, if any, or the
    /// size of the value otherwise.
    fn write_subspace_val(
        &mut self,
        height: BlockHeight,
        key: &Key,
        value: impl AsRef<[u8]>,
    ) -> Result<i64>;

    /// Delete the value with the given height and account subspace key from the
    /// DB. Returns the size of the removed value, if any, 0 if no previous
    /// value was found.
    fn delete_subspace_val(
        &mut self,
        height: BlockHeight,
        key: &Key,
    ) -> Result<i64>;

    /// Start write batch.
    fn batch() -> Self::WriteBatch;

    /// Execute write batch.
    fn exec_batch(&mut self, batch: Self::WriteBatch) -> Result<()>;

    /// Batch write the value with the given height and account subspace key to
    /// the DB. Returns the size difference from previous value, if any, or
    /// the size of the value otherwise.
    fn batch_write_subspace_val(
        &self,
        batch: &mut Self::WriteBatch,
        height: BlockHeight,
        key: &Key,
        value: impl AsRef<[u8]>,
    ) -> Result<i64>;

    /// Batch delete the value with the given height and account subspace key
    /// from the DB. Returns the size of the removed value, if any, 0 if no
    /// previous value was found.
    fn batch_delete_subspace_val(
        &self,
        batch: &mut Self::WriteBatch,
        height: BlockHeight,
        key: &Key,
    ) -> Result<i64>;
}

/// A database prefix iterator.
pub trait DBIter<'iter> {
    /// The concrete type of the iterator
    type PrefixIter: Debug + Iterator<Item = (String, Vec<u8>, u64)>;

    /// Read account subspace key value pairs with the given prefix from the DB,
    /// ordered by the storage keys.
    fn iter_prefix(&'iter self, prefix: &Key) -> Self::PrefixIter;

    /// Read account subspace key value pairs with the given prefix from the DB,
    /// reverse ordered by the storage keys.
    fn rev_iter_prefix(&'iter self, prefix: &Key) -> Self::PrefixIter;
}

/// Atomic batch write.
pub trait DBWriteBatch {
    /// Insert a value into the database under the given key.
    fn put<K, V>(&mut self, key: K, value: V)
    where
        K: AsRef<[u8]>,
        V: AsRef<[u8]>;

    /// Removes the database entry for key. Does nothing if the key was not
    /// found.
    fn delete<K: AsRef<[u8]>>(&mut self, key: K);
}

impl<D, H> Storage<D, H>
where
    D: DB + for<'iter> DBIter<'iter>,
    H: StorageHasher,
{
    /// open up a new instance of the storage given path to db and chain id
    pub fn open(
        db_path: impl AsRef<std::path::Path>,
        chain_id: ChainId,
        cache: Option<&D::Cache>,
    ) -> Self {
        let block = BlockStorage {
            tree: MerkleTree::default(),
            hash: BlockHash::default(),
            height: BlockHeight::default(),
            epoch: Epoch::default(),
            pred_epochs: Epochs::default(),
        };
        Storage::<D, H> {
            db: D::open(db_path, cache),
            chain_id,
            block,
            header: None,
            last_height: BlockHeight(0),
            last_epoch: Epoch::default(),
            next_epoch_min_start_height: BlockHeight::default(),
            next_epoch_min_start_time: DateTimeUtc::now(),
            address_gen: EstablishedAddressGen::new(
                "Privacy is a function of liberty.",
            ),
            #[cfg(feature = "ferveo-tpke")]
            tx_queue: TxQueue::default(),
        }
    }

    /// Load the full state at the last committed height, if any. Returns the
    /// Merkle root hash and the height of the committed block.
    pub fn load_last_state(&mut self) -> Result<()> {
        if let Some(BlockStateRead {
            merkle_tree_stores,
            hash,
            height,
            epoch,
            pred_epochs,
            next_epoch_min_start_height,
            next_epoch_min_start_time,
            address_gen,
            #[cfg(feature = "ferveo-tpke")]
            tx_queue,
        }) = self.db.read_last_block()?
        {
            self.block.tree = MerkleTree::new(merkle_tree_stores);
            self.block.hash = hash;
            self.block.height = height;
            self.block.epoch = epoch;
            self.block.pred_epochs = pred_epochs;
            self.last_height = height;
            self.last_epoch = epoch;
            self.next_epoch_min_start_height = next_epoch_min_start_height;
            self.next_epoch_min_start_time = next_epoch_min_start_time;
            self.address_gen = address_gen;
            #[cfg(feature = "ferveo-tpke")]
            {
                self.tx_queue = tx_queue;
            }
            tracing::debug!("Loaded storage from DB");
        } else {
            tracing::info!("No state could be found");
        }
        Ok(())
    }

    /// Returns the Merkle root hash and the height of the committed block. If
    /// no block exists, returns None.
    pub fn get_state(&self) -> Option<(MerkleRoot, u64)> {
        if self.block.height.0 != 0 {
            Some((self.block.tree.root(), self.block.height.0))
        } else {
            None
        }
    }

    /// Persist the current block's state to the database
    pub fn commit(&mut self) -> Result<()> {
        let state = BlockStateWrite {
            merkle_tree_stores: self.block.tree.stores(),
            header: self.header.as_ref(),
            hash: &self.block.hash,
            height: self.block.height,
            epoch: self.block.epoch,
            pred_epochs: &self.block.pred_epochs,
            next_epoch_min_start_height: self.next_epoch_min_start_height,
            next_epoch_min_start_time: self.next_epoch_min_start_time,
            address_gen: &self.address_gen,
            #[cfg(feature = "ferveo-tpke")]
            tx_queue: &self.tx_queue,
        };
        self.db.write_block(state)?;
        self.last_height = self.block.height;
        self.last_epoch = self.block.epoch;
        self.header = None;
        Ok(())
    }

    /// Find the root hash of the merkle tree
    pub fn merkle_root(&self) -> MerkleRoot {
        self.block.tree.root()
    }

    /// Check if the given key is present in storage. Returns the result and the
    /// gas cost.
    pub fn has_key(&self, key: &Key) -> Result<(bool, u64)> {
        Ok((self.block.tree.has_key(key)?, key.len() as _))
    }

    /// Returns a value from the specified subspace and the gas cost
    pub fn read(&self, key: &Key) -> Result<(Option<Vec<u8>>, u64)> {
        tracing::debug!("storage read key {}", key);
        let (present, gas) = self.has_key(key)?;
        if !present {
            return Ok((None, gas));
        }

        match self.db.read_subspace_val(key)? {
            Some(v) => {
                let gas = key.len() + v.len();
                Ok((Some(v), gas as _))
            }
            None => Ok((None, key.len() as _)),
        }
    }

    /// Returns a value from the specified subspace at the given height and the
    /// gas cost
    pub fn read_with_height(
        &self,
        key: &Key,
        height: BlockHeight,
    ) -> Result<(Option<Vec<u8>>, u64)> {
        if height >= self.last_height {
            self.read(key)
        } else {
            match self.db.read_subspace_val_with_height(
                key,
                height,
                self.last_height,
            )? {
                Some(v) => {
                    let gas = key.len() + v.len();
                    Ok((Some(v), gas as _))
                }
                None => Ok((None, key.len() as _)),
            }
        }
    }

    /// Returns a prefix iterator, ordered by storage keys, and the gas cost
    pub fn iter_prefix(
        &self,
        prefix: &Key,
    ) -> (<D as DBIter<'_>>::PrefixIter, u64) {
        (self.db.iter_prefix(prefix), prefix.len() as _)
    }

    /// Returns a prefix iterator, reverse ordered by storage keys, and the gas
    /// cost
    pub fn rev_iter_prefix(
        &self,
        prefix: &Key,
    ) -> (<D as DBIter<'_>>::PrefixIter, u64) {
        (self.db.rev_iter_prefix(prefix), prefix.len() as _)
    }

    /// Write a value to the specified subspace and returns the gas cost and the
    /// size difference
    pub fn write(
        &mut self,
        key: &Key,
        value: impl Into<MerkleValue>,
    ) -> Result<(u64, i64)> {
        let value: MerkleValue = value.into();
        tracing::debug!("storage write key {}", key,);
        self.block.tree.update(key, value.clone())?;

        let bytes = value.to_bytes();
        let gas = key.len() + bytes.len();
        let size_diff =
            self.db.write_subspace_val(self.block.height, key, bytes)?;
        Ok((gas as _, size_diff))
    }

    /// Delete the specified subspace and returns the gas cost and the size
    /// difference
    pub fn delete(&mut self, key: &Key) -> Result<(u64, i64)> {
        // Note that this method is the same as `StorageWrite::delete`,
        // but with gas and storage bytes len diff accounting
        let mut deleted_bytes_len = 0;
        if self.has_key(key)?.0 {
            self.block.tree.delete(key)?;
            deleted_bytes_len =
                self.db.delete_subspace_val(self.block.height, key)?;
        }
        let gas = key.len() + deleted_bytes_len as usize;
        Ok((gas as _, deleted_bytes_len))
    }

    /// Set the block header.
    /// The header is not in the Merkle tree as it's tracked by Tendermint.
    /// Hence, we don't update the tree when this is set.
    pub fn set_header(&mut self, header: Header) -> Result<()> {
        self.header = Some(header);
        Ok(())
    }

    /// Block data is in the Merkle tree as it's tracked by Tendermint in the
    /// block header. Hence, we don't update the tree when this is set.
    pub fn begin_block(
        &mut self,
        hash: BlockHash,
        height: BlockHeight,
    ) -> Result<()> {
        self.block.hash = hash;
        self.block.height = height;
        Ok(())
    }

    /// Get a validity predicate for the given account address and the gas cost
    /// for reading it.
    pub fn validity_predicate(
        &self,
        addr: &Address,
    ) -> Result<(Option<Vec<u8>>, u64)> {
        let key = Key::validity_predicate(addr);
        self.read(&key)
    }

    #[allow(dead_code)]
    /// Check if the given address exists on chain and return the gas cost.
    pub fn exists(&self, addr: &Address) -> Result<(bool, u64)> {
        let key = Key::validity_predicate(addr);
        self.has_key(&key)
    }

    /// Get the chain ID as a raw string
    pub fn get_chain_id(&self) -> (String, u64) {
        (self.chain_id.to_string(), CHAIN_ID_LENGTH as _)
    }

    /// Get the current (yet to be committed) block height
    pub fn get_block_height(&self) -> (BlockHeight, u64) {
        (self.block.height, MIN_STORAGE_GAS)
    }

    /// Get the current (yet to be committed) block hash
    pub fn get_block_hash(&self) -> (BlockHash, u64) {
        (self.block.hash.clone(), BLOCK_HASH_LENGTH as _)
    }

    /// Get a Tendermint-compatible existence proof.
    ///
    /// Proofs from the Ethereum bridge pool are not
    /// Tendermint-compatible. Requesting for a key
    /// belonging to the bridge pool will cause this
    /// method to error.
    pub fn get_existence_proof(
        &self,
        key: &Key,
        value: MerkleValue,
        height: BlockHeight,
    ) -> Result<Proof> {
        if height >= self.get_block_height().0 {
            if let MembershipProof::ICS23(proof) = self
                .block
                .tree
                .get_sub_tree_existence_proof(array::from_ref(key), vec![value])
                .map_err(Error::MerkleTreeError)?
            {
                self.block
                    .tree
                    .get_tendermint_proof(key, proof)
                    .map_err(Error::MerkleTreeError)
            } else {
                Err(Error::MerkleTreeError(MerkleTreeError::TendermintProof))
            }
        } else {
            match self.db.read_merkle_tree_stores(height)? {
                Some(stores) => {
                    let tree = MerkleTree::<H>::new(stores);
                    if let MembershipProof::ICS23(proof) = tree
                        .get_sub_tree_existence_proof(
                            array::from_ref(key),
                            vec![value],
                        )
                        .map_err(Error::MerkleTreeError)?
                    {
                        tree.get_tendermint_proof(key, proof)
                            .map_err(Error::MerkleTreeError)
                    } else {
                        Err(Error::MerkleTreeError(
                            MerkleTreeError::TendermintProof,
                        ))
                    }
                }
                None => Err(Error::NoMerkleTree { height }),
            }
        }
    }

    /// Get the non-existence proof
    pub fn get_non_existence_proof(
        &self,
        key: &Key,
        height: BlockHeight,
    ) -> Result<Proof> {
        if height >= self.get_block_height().0 {
            Ok(self.block.tree.get_non_existence_proof(key)?)
        } else {
            match self.db.read_merkle_tree_stores(height)? {
                Some(stores) => Ok(MerkleTree::<H>::new(stores)
                    .get_non_existence_proof(key)?),
                None => Err(Error::NoMerkleTree { height }),
            }
        }
    }

    /// Get the current (yet to be committed) block epoch
    pub fn get_current_epoch(&self) -> (Epoch, u64) {
        (self.block.epoch, MIN_STORAGE_GAS)
    }

    /// Get the epoch of the last committed block
    pub fn get_last_epoch(&self) -> (Epoch, u64) {
        (self.last_epoch, MIN_STORAGE_GAS)
    }

    /// Initialize the first epoch. The first epoch begins at genesis time.
    pub fn init_genesis_epoch(
        &mut self,
        initial_height: BlockHeight,
        genesis_time: DateTimeUtc,
        parameters: &Parameters,
    ) -> Result<()> {
        let EpochDuration {
            min_num_of_blocks,
            min_duration,
        } = parameters.epoch_duration;
        self.next_epoch_min_start_height = initial_height + min_num_of_blocks;
        self.next_epoch_min_start_time = genesis_time + min_duration;
        self.update_epoch_in_merkle_tree()
    }

    /// Get the block header
    pub fn get_block_header(
        &self,
        height: Option<BlockHeight>,
    ) -> Result<(Option<Header>, u64)> {
        match height {
            Some(h) if h == self.get_block_height().0 => {
                Ok((self.header.clone(), MIN_STORAGE_GAS))
            }
            Some(h) => match self.db.read_block_header(h)? {
                Some(header) => {
                    let gas = header.encoded_len() as u64;
                    Ok((Some(header), gas))
                }
                None => Ok((None, MIN_STORAGE_GAS)),
            },
            None => Ok((self.header.clone(), MIN_STORAGE_GAS)),
        }
    }

    /// Initialize a new epoch when the current epoch is finished. Returns
    /// `true` on a new epoch.
    pub fn update_epoch(
        &mut self,
        height: BlockHeight,
        time: DateTimeUtc,
    ) -> Result<bool> {
        let (parameters, _gas) =
            parameters::read(self).expect("Couldn't read protocol parameters");

        // Check if the current epoch is over
        let new_epoch = height >= self.next_epoch_min_start_height
            && time >= self.next_epoch_min_start_time;
        if new_epoch {
            // Begin a new epoch
            self.block.epoch = self.block.epoch.next();
            let EpochDuration {
                min_num_of_blocks,
                min_duration,
            } = parameters.epoch_duration;
            self.next_epoch_min_start_height = height + min_num_of_blocks;
            self.next_epoch_min_start_time = time + min_duration;
            // TODO put this into PoS parameters and pass it to tendermint
            // `consensus_params` on `InitChain` and `EndBlock`
            let evidence_max_age_num_blocks: u64 = 100000;
            self.block
                .pred_epochs
                .new_epoch(height, evidence_max_age_num_blocks);
            tracing::info!("Began a new epoch {}", self.block.epoch);
        }
        self.update_epoch_in_merkle_tree()?;
        Ok(new_epoch)
    }

    /// Update the merkle tree with epoch data
    fn update_epoch_in_merkle_tree(&mut self) -> Result<()> {
        let key_prefix: Key =
            Address::Internal(InternalAddress::PoS).to_db_key().into();

        let key = key_prefix
            .push(&"epoch_start_height".to_string())
            .map_err(Error::KeyError)?;
        self.block
            .tree
            .update(&key, types::encode(&self.next_epoch_min_start_height))?;

        let key = key_prefix
            .push(&"epoch_start_time".to_string())
            .map_err(Error::KeyError)?;
        self.block
            .tree
            .update(&key, types::encode(&self.next_epoch_min_start_time))?;

        let key = key_prefix
            .push(&"current_epoch".to_string())
            .map_err(Error::KeyError)?;
        self.block
            .tree
            .update(&key, types::encode(&self.block.epoch))?;
        Ok(())
    }

    /// Start write batch.
    fn batch() -> D::WriteBatch {
        D::batch()
    }

    /// Execute write batch.
    fn exec_batch(&mut self, batch: D::WriteBatch) -> Result<()> {
        self.db.exec_batch(batch)
    }

    /// Batch write the value with the given height and account subspace key to
    /// the DB. Returns the size difference from previous value, if any, or
    /// the size of the value otherwise.
    fn batch_write_subspace_val(
        &mut self,
        batch: &mut D::WriteBatch,
        key: &Key,
        value: impl AsRef<[u8]>,
    ) -> Result<i64> {
        let value = value.as_ref();
        self.block.tree.update(key, value)?;
        self.db
            .batch_write_subspace_val(batch, self.block.height, key, value)
    }

    /// Batch delete the value with the given height and account subspace key
    /// from the DB. Returns the size of the removed value, if any, 0 if no
    /// previous value was found.
    fn batch_delete_subspace_val(
        &mut self,
        batch: &mut D::WriteBatch,
        key: &Key,
    ) -> Result<i64> {
        self.block.tree.delete(key)?;
        self.db
            .batch_delete_subspace_val(batch, self.block.height, key)
    }
}

impl<'iter, D, H> StorageRead<'iter> for Storage<D, H>
where
    D: DB + for<'iter_> DBIter<'iter_>,
    H: StorageHasher,
{
    type PrefixIter = <D as DBIter<'iter>>::PrefixIter;

    fn read_bytes(
        &self,
        key: &crate::types::storage::Key,
    ) -> std::result::Result<Option<Vec<u8>>, storage_api::Error> {
        self.db.read_subspace_val(key).into_storage_result()
    }

    fn has_key(
        &self,
        key: &crate::types::storage::Key,
    ) -> std::result::Result<bool, storage_api::Error> {
        self.block.tree.has_key(key).into_storage_result()
    }

    fn iter_prefix(
        &'iter self,
        prefix: &crate::types::storage::Key,
    ) -> std::result::Result<Self::PrefixIter, storage_api::Error> {
        Ok(self.db.iter_prefix(prefix))
    }

    fn rev_iter_prefix(
        &'iter self,
        prefix: &crate::types::storage::Key,
    ) -> std::result::Result<Self::PrefixIter, storage_api::Error> {
        Ok(self.db.rev_iter_prefix(prefix))
    }

    fn iter_next(
        &self,
        iter: &mut Self::PrefixIter,
    ) -> std::result::Result<Option<(String, Vec<u8>)>, storage_api::Error>
    {
        Ok(iter.next().map(|(key, val, _gas)| (key, val)))
    }

    fn get_chain_id(&self) -> std::result::Result<String, storage_api::Error> {
        Ok(self.chain_id.to_string())
    }

    fn get_block_height(
        &self,
    ) -> std::result::Result<BlockHeight, storage_api::Error> {
        Ok(self.block.height)
    }

    fn get_block_hash(
        &self,
    ) -> std::result::Result<BlockHash, storage_api::Error> {
        Ok(self.block.hash.clone())
    }

    fn get_block_epoch(
        &self,
    ) -> std::result::Result<Epoch, storage_api::Error> {
        Ok(self.block.epoch)
    }
}

impl<D, H> StorageWrite for Storage<D, H>
where
    D: DB + for<'iter> DBIter<'iter>,
    H: StorageHasher,
{
    fn write_bytes(
        &mut self,
        key: &crate::types::storage::Key,
        val: impl AsRef<[u8]>,
    ) -> storage_api::Result<()> {
        // Note that this method is the same as `Storage::write`, but without
        // gas and storage bytes len diff accounting, because it can only be
        // used by the protocol that has a direct mutable access to storage
        let val = val.as_ref();
        self.block.tree.update(key, &val).into_storage_result()?;
        let _ = self
            .db
            .write_subspace_val(self.block.height, key, val)
            .into_storage_result()?;
        Ok(())
    }

    fn delete(
        &mut self,
        key: &crate::types::storage::Key,
    ) -> storage_api::Result<()> {
        // Note that this method is the same as `Storage::delete`, but without
        // gas and storage bytes len diff accounting, because it can only be
        // used by the protocol that has a direct mutable access to storage
        self.block.tree.delete(key).into_storage_result()?;
        let _ = self
            .db
            .delete_subspace_val(self.block.height, key)
            .into_storage_result()?;
        Ok(())
    }
}

impl<D, H> StorageWrite for &mut Storage<D, H>
where
    D: DB + for<'iter> DBIter<'iter>,
    H: StorageHasher,
{
    fn write<T: borsh::BorshSerialize>(
        &mut self,
        key: &crate::types::storage::Key,
        val: T,
    ) -> storage_api::Result<()> {
        let val = val.try_to_vec().unwrap();
        self.write_bytes(key, val)
    }

    fn write_bytes(
        &mut self,
        key: &crate::types::storage::Key,
        val: impl AsRef<[u8]>,
    ) -> storage_api::Result<()> {
        let _ = self
            .db
            .write_subspace_val(self.block.height, key, val)
            .into_storage_result()?;
        Ok(())
    }

    fn delete(
        &mut self,
        key: &crate::types::storage::Key,
    ) -> storage_api::Result<()> {
        let _ = self
            .db
            .delete_subspace_val(self.block.height, key)
            .into_storage_result()?;
        Ok(())
    }
}

pub struct GetActiveValidatorsIntoIter {
    epoch: Epoch,
    sets: ValidatorSets,
}

impl<'it> IntoIterator for &'it GetActiveValidatorsIntoIter {
    type IntoIter = btree_set::Iter<'it, WeightedValidator<Address>>;
    type Item = &'it WeightedValidator<Address>;

    fn into_iter(self) -> Self::IntoIter {
        self.sets
            .get(self.epoch)
            .expect("Validators for an epoch should be known")
            .active
            .iter()
    }
}

impl<'active_validators, D, H> QueriesExt<'active_validators> for Storage<D, H>
where
    D: DB + for<'iter> DBIter<'iter>,
    H: StorageHasher,
{
    type ActiveValidatorsIter = GetActiveValidatorsIntoIter;

    fn get_active_validators(
        &self,
        epoch: Option<Epoch>,
    ) -> GetActiveValidatorsIntoIter {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        let sets = self.read_validator_set();
        GetActiveValidatorsIntoIter { sets, epoch }
    }

    fn get_total_voting_power(&self, epoch: Option<Epoch>) -> VotingPower {
        self.get_active_validators(epoch)
            .into_iter()
            .map(|validator| u64::from(validator.voting_power))
            .sum::<u64>()
            .into()
    }

    fn get_balance(&self, token: &Address, owner: &Address) -> Amount {
        let balance = storage_api::StorageRead::read(
            self,
            &token::balance_key(token, owner),
        );
        // Storage read must not fail, but there might be no value, in which
        // case default (0) is returned
        balance
            .expect("Storage read in the protocol must not fail")
            .unwrap_or_default()
    }

    fn get_evidence_params(
        &self,
        epoch_duration: &EpochDuration,
        pos_params: &PosParams,
    ) -> EvidenceParams {
        // Minimum number of epochs before tokens are unbonded and can be
        // withdrawn
        let len_before_unbonded =
            std::cmp::max(pos_params.unbonding_len as i64 - 1, 0);
        let max_age_num_blocks: i64 =
            epoch_duration.min_num_of_blocks as i64 * len_before_unbonded;
        let min_duration_secs = epoch_duration.min_duration.0 as i64;
        let max_age_duration = Some(protobuf::Duration {
            seconds: min_duration_secs * len_before_unbonded,
            nanos: 0,
        });
        EvidenceParams {
            max_age_num_blocks,
            max_age_duration,
            ..EvidenceParams::default()
        }
    }

    fn get_validator_from_protocol_pk(
        &self,
        pk: &key::common::PublicKey,
        epoch: Option<Epoch>,
    ) -> queries::Result<TendermintValidator<EllipticCurve>> {
        let pk_bytes = pk
            .try_to_vec()
            .expect("Serializing public key should not fail");
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        self.get_active_validators(Some(epoch))
            .into_iter()
            .find(|validator| {
                let pk_key = key::protocol_pk_key(&validator.address);
                match self.read(&pk_key) {
                    Ok((Some(bytes), _)) => bytes == pk_bytes,
                    _ => false,
                }
            })
            .map(|validator| {
                let dkg_key =
                    key::dkg_session_keys::dkg_pk_key(&validator.address);
                let bytes = self
                    .read(&dkg_key)
                    .expect("Validator should have public dkg key")
                    .0
                    .expect("Validator should have public dkg key");
                let dkg_publickey =
                    &<DkgPublicKey as BorshDeserialize>::deserialize(
                        &mut bytes.as_ref(),
                    )
                    .expect(
                        "DKG public key in storage should be deserializable",
                    );
                TendermintValidator {
                    power: validator.voting_power.into(),
                    address: validator.address.to_string(),
                    public_key: dkg_publickey.into(),
                }
            })
            .ok_or_else(|| {
                queries::Error::NotValidatorKey(pk.to_string(), epoch)
            })
    }

    fn get_validator_from_address(
        &self,
        address: &Address,
        epoch: Option<Epoch>,
    ) -> queries::Result<(VotingPower, key::common::PublicKey)> {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        self.get_active_validators(Some(epoch))
            .into_iter()
            .find(|validator| address == &validator.address)
            .map(|validator| {
                let protocol_pk_key = key::protocol_pk_key(&validator.address);
                let bytes = self
                    .read(&protocol_pk_key)
                    .expect("Validator should have public protocol key")
                    .0
                    .expect("Validator should have public protocol key");
                let protocol_pk: key::common::PublicKey =
                    BorshDeserialize::deserialize(&mut bytes.as_ref()).expect(
                        "Protocol public key in storage should be \
                         deserializable",
                    );
                (validator.voting_power, protocol_pk)
            })
            .ok_or_else(|| {
                queries::Error::NotValidatorAddress(address.clone(), epoch)
            })
    }

    fn get_validator_from_tm_address(
        &self,
        tm_address: &[u8],
        epoch: Option<Epoch>,
    ) -> queries::Result<Address> {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        let validator_raw_hash = core::str::from_utf8(tm_address)
            .map_err(|_| queries::Error::InvalidTMAddress)?;
        self.read_validator_address_raw_hash(&validator_raw_hash)
            .ok_or_else(|| {
                queries::Error::NotValidatorKeyHash(
                    validator_raw_hash.to_string(),
                    epoch,
                )
            })
    }

    #[cfg(feature = "abcipp")]
    fn can_send_validator_set_update(&self, _can_send: SendValsetUpd) -> bool {
        // TODO: implement this method for ABCI++; should only be able to send
        // a validator set update at the second block of an epoch
        true
    }

    #[cfg(not(feature = "abcipp"))]
    fn can_send_validator_set_update(&self, can_send: SendValsetUpd) -> bool {
        // when checking vote extensions in Prepare
        // and ProcessProposal, we simply return true
        if matches!(can_send, SendValsetUpd::AtPrevHeight) {
            return true;
        }

        let current_decision_height = self.get_current_decision_height();

        // NOTE: the first stored height in `fst_block_heights_of_each_epoch`
        // is 0, because of a bug (should be 1), so this code needs to
        // handle that case
        //
        // we can remove this check once that's fixed
        match current_decision_height {
            BlockHeight(1) => return false,
            BlockHeight(2) => return true,
            _ => (),
        }

        let fst_heights_of_each_epoch =
            self.block.pred_epochs.first_block_heights();

        fst_heights_of_each_epoch
            .last()
            .map(|&h| {
                let second_height_of_epoch = h + 1;
                current_decision_height == second_height_of_epoch
            })
            .unwrap_or(false)
    }

    #[inline]
    fn get_epoch(&self, height: BlockHeight) -> Option<Epoch> {
        self.block.pred_epochs.get_epoch(height)
    }

    #[inline]
    fn get_current_decision_height(&self) -> BlockHeight {
        self.last_height + 1
    }

    #[inline]
    fn get_ethbridge_from_namada_addr(
        &self,
        validator: &Address,
        epoch: Option<Epoch>,
    ) -> Option<EthAddress> {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        self.read_validator_eth_hot_key(validator)
            .as_ref()
            .and_then(|epk| epk.get(epoch).and_then(|pk| pk.try_into().ok()))
    }

    #[inline]
    fn get_ethgov_from_namada_addr(
        &self,
        validator: &Address,
        epoch: Option<Epoch>,
    ) -> Option<EthAddress> {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        self.read_validator_eth_cold_key(validator)
            .as_ref()
            .and_then(|epk| epk.get(epoch).and_then(|pk| pk.try_into().ok()))
    }

    #[inline]
    fn get_active_eth_addresses<'db>(
        &'db self,
        epoch: Option<Epoch>,
    ) -> Box<dyn Iterator<Item = (EthAddrBook, Address, VotingPower)> + 'db>
    {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        Box::new(
            self.get_active_validators(Some(epoch))
                .into_iter()
                .cloned()
                .map(move |validator| {
                    let hot_key_addr = self
                        .get_ethbridge_from_namada_addr(
                            &validator.address,
                            Some(epoch),
                        )
                        .expect(
                            "All Namada validators should have an Ethereum \
                             bridge key",
                        );
                    let cold_key_addr = self
                        .get_ethgov_from_namada_addr(
                            &validator.address,
                            Some(epoch),
                        )
                        .expect(
                            "All Namada validators should have an Ethereum \
                             governance key",
                        );
                    let eth_addr_book = EthAddrBook {
                        hot_key_addr,
                        cold_key_addr,
                    };
                    (eth_addr_book, validator.address, validator.voting_power)
                }),
        )
    }
}

impl From<MerkleTreeError> for Error {
    fn from(error: MerkleTreeError) -> Self {
        Self::MerkleTreeError(error)
    }
}

/// Helpers for testing components that depend on storage
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use super::mockdb::MockDB;
    use super::*;
    use crate::ledger::storage::traits::Sha256Hasher;
    /// Storage with a mock DB for testing
    pub type TestStorage = Storage<MockDB, Sha256Hasher>;

    impl Default for TestStorage {
        fn default() -> Self {
            let chain_id = ChainId::default();
            let tree = MerkleTree::default();
            let block = BlockStorage {
                tree,
                hash: BlockHash::default(),
                height: BlockHeight::default(),
                epoch: Epoch::default(),
                pred_epochs: Epochs::default(),
            };
            Self {
                db: MockDB::default(),
                chain_id,
                block,
                header: None,
                last_height: BlockHeight(0),
                last_epoch: Epoch::default(),
                next_epoch_min_start_height: BlockHeight::default(),
                next_epoch_min_start_time: DateTimeUtc::now(),
                address_gen: EstablishedAddressGen::new(
                    "Test address generator seed",
                ),
                #[cfg(feature = "ferveo-tpke")]
                tx_queue: TxQueue::default(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use proptest::prelude::*;

    use super::testing::*;
    use super::*;
    use crate::ledger::parameters::{self, Parameters};
    use crate::types::time::{self, Duration};

    prop_compose! {
        /// Setup test input data with arbitrary epoch duration, epoch start
        /// height and time, and a block height and time that are greater than
        /// the epoch start height and time, and the change to be applied to
        /// the epoch duration parameters.
        fn arb_and_epoch_duration_start_and_block()
        (
            start_height in 0..1000_u64,
            start_time in 0..10000_i64,
            min_num_of_blocks in 1..10_u64,
            min_duration in 1..100_i64,
            max_expected_time_per_block in 1..100_i64,
        )
        (
            min_num_of_blocks in Just(min_num_of_blocks),
            min_duration in Just(min_duration),
            max_expected_time_per_block in Just(max_expected_time_per_block),
            start_height in Just(start_height),
            start_time in Just(start_time),
            block_height in start_height + 1..(start_height + 2 * min_num_of_blocks as u64),
            block_time in start_time + 1..(start_time + 2 * min_duration),
            // Delta will be applied on the `min_num_of_blocks` parameter
            min_blocks_delta in -(min_num_of_blocks as i64 - 1)..5,
            // Delta will be applied on the `min_duration` parameter
            min_duration_delta in -(min_duration as i64 - 1)..50,
            // Delta will be applied on the `max_expected_time_per_block` parameter
            max_time_per_block_delta in -(max_expected_time_per_block as i64 - 1)..50,
        ) -> (EpochDuration, i64, BlockHeight, DateTimeUtc, BlockHeight, DateTimeUtc,
                i64, i64, i64) {
            let epoch_duration = EpochDuration {
                min_num_of_blocks,
                min_duration: Duration::seconds(min_duration).into(),
            };
            (epoch_duration, max_expected_time_per_block,
                BlockHeight(start_height), Utc.timestamp(start_time, 0).into(),
                BlockHeight(block_height), Utc.timestamp(block_time, 0).into(),
                min_blocks_delta, min_duration_delta, max_time_per_block_delta)
        }
    }

    proptest! {
        /// Test that:
        /// 1. When the minimum blocks have been created since the epoch
        ///    start height and minimum time passed since the epoch start time,
        ///    a new epoch must start.
        /// 2. When the epoch duration parameters change, the current epoch's
        ///    duration doesn't change, but the next one does.
        #[test]
        fn update_epoch_after_its_duration(
            (epoch_duration, max_expected_time_per_block, start_height, start_time, block_height, block_time,
            min_blocks_delta, min_duration_delta, max_time_per_block_delta)
            in arb_and_epoch_duration_start_and_block())
        {
            let mut storage = TestStorage {
                next_epoch_min_start_height:
                    start_height + epoch_duration.min_num_of_blocks,
                next_epoch_min_start_time:
                    start_time + epoch_duration.min_duration,
                ..Default::default()
            };
            let mut parameters = Parameters {
                epoch_duration: epoch_duration.clone(),
                max_expected_time_per_block: Duration::seconds(max_expected_time_per_block).into(),
                vp_whitelist: vec![],
                tx_whitelist: vec![]
            };
            parameters.init_storage(&mut storage);

            let epoch_before = storage.last_epoch;
            assert_eq!(epoch_before, storage.block.epoch);

            // Try to apply the epoch update
            storage.update_epoch(block_height, block_time).unwrap();

            // Test for 1.
            if block_height.0 - start_height.0
                >= epoch_duration.min_num_of_blocks as u64
                && time::duration_passed(
                    block_time,
                    start_time,
                    epoch_duration.min_duration,
                )
            {
                assert_eq!(storage.block.epoch, epoch_before.next());
                assert_eq!(storage.next_epoch_min_start_height,
                    block_height + epoch_duration.min_num_of_blocks);
                assert_eq!(storage.next_epoch_min_start_time,
                    block_time + epoch_duration.min_duration);
                assert_eq!(
                    storage.block.pred_epochs.get_epoch(BlockHeight(block_height.0 - 1)),
                    Some(epoch_before));
                assert_eq!(
                    storage.block.pred_epochs.get_epoch(block_height),
                    Some(epoch_before.next()));
            } else {
                assert_eq!(storage.block.epoch, epoch_before);
                assert_eq!(
                    storage.block.pred_epochs.get_epoch(BlockHeight(block_height.0 - 1)),
                    Some(epoch_before));
                assert_eq!(
                    storage.block.pred_epochs.get_epoch(block_height),
                    Some(epoch_before));
            }
            // Last epoch should only change when the block is committed
            assert_eq!(storage.last_epoch, epoch_before);

            // Update the epoch duration parameters
            parameters.epoch_duration.min_num_of_blocks =
                (parameters.epoch_duration.min_num_of_blocks as i64 + min_blocks_delta) as u64;
            let min_duration: i64 = parameters.epoch_duration.min_duration.0 as _;
            parameters.epoch_duration.min_duration =
                Duration::seconds(min_duration + min_duration_delta).into();
            parameters.max_expected_time_per_block =
                Duration::seconds(max_expected_time_per_block + max_time_per_block_delta).into();
            parameters::update_max_expected_time_per_block_parameter(&mut storage, &parameters.max_expected_time_per_block).unwrap();
            parameters::update_epoch_parameter(&mut storage, &parameters.epoch_duration).unwrap();

            // Test for 2.
            let epoch_before = storage.block.epoch;
            let height_of_update = storage.next_epoch_min_start_height.0 ;
            let time_of_update = storage.next_epoch_min_start_time;
            let height_before_update = BlockHeight(height_of_update - 1);
            let height_of_update = BlockHeight(height_of_update);
            let time_before_update = time_of_update - Duration::seconds(1);

            // No update should happen before both epoch duration conditions are
            // satisfied
            storage.update_epoch(height_before_update, time_before_update).unwrap();
            assert_eq!(storage.block.epoch, epoch_before);
            storage.update_epoch(height_of_update, time_before_update).unwrap();
            assert_eq!(storage.block.epoch, epoch_before);
            storage.update_epoch(height_before_update, time_of_update).unwrap();
            assert_eq!(storage.block.epoch, epoch_before);

            // Update should happen at this or after this height and time
            storage.update_epoch(height_of_update, time_of_update).unwrap();
            assert_eq!(storage.block.epoch, epoch_before.next());
            // The next epoch's minimum duration should change
            assert_eq!(storage.next_epoch_min_start_height,
                height_of_update + parameters.epoch_duration.min_num_of_blocks);
            assert_eq!(storage.next_epoch_min_start_time,
                time_of_update + parameters.epoch_duration.min_duration);
        }
    }
}
