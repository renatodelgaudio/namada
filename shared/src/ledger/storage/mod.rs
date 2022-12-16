//! Ledger's state storage with key-value backed store and a merkle tree

pub mod write_log;

#[cfg(any(test, feature = "testing"))]
pub use namada_core::ledger::storage::mockdb;
pub use namada_core::ledger::storage::{traits, *};
use namada_core::ledger::storage_api::{ResultExt, StorageRead, StorageWrite};
use namada_core::types::storage::Key;

use self::write_log::WriteLog;

pub struct StorageWithWriteLog<'a, D, H>
where
    D: DB + for<'iter> DBIter<'iter>,
    H: StorageHasher,
{
    pub storage: &'a mut Storage<D, H>,
    pub write_log: &'a mut WriteLog,
}

impl<'iter, D, H> StorageRead<'iter> for StorageWithWriteLog<'_, D, H>
where
    D: DB + for<'iter_> DBIter<'iter_>,
    H: StorageHasher,
{
    type PrefixIter = <D as DBIter<'iter>>::PrefixIter;

    fn read_bytes(
        &self,
        key: &namada_core::types::storage::Key,
    ) -> namada_core::ledger::storage_api::Result<Option<Vec<u8>>> {
        // try to read from the write log first
        let (log_val, _gas) = self.write_log.read(&key);
        match log_val {
            Some(&write_log::StorageModification::Write { ref value }) => {
                Ok(Some(value.clone()))
            }
            Some(&write_log::StorageModification::Delete) => return Ok(None),
            Some(&write_log::StorageModification::InitAccount {
                ref vp,
                ..
            }) => Ok(Some(vp.clone())),
            Some(&write_log::StorageModification::Temp { ref value }) => {
                Ok(Some(value.clone()))
            }
            None => {
                // when not found in write log, try to read from the storage
                StorageRead::read_bytes(self.storage, key)
            }
        }
    }

    fn has_key(
        &self,
        key: &namada_core::types::storage::Key,
    ) -> namada_core::ledger::storage_api::Result<bool> {
        // try to read from the write log first
        let (log_val, _gas) = self.write_log.read(&key);
        match log_val {
            Some(&write_log::StorageModification::Write { .. })
            | Some(&write_log::StorageModification::InitAccount { .. })
            | Some(&write_log::StorageModification::Temp { .. }) => Ok(true),
            Some(&write_log::StorageModification::Delete) => {
                // the given key has been deleted
                Ok(false)
            }
            None => {
                // when not found in write log, try to check the storage
                StorageRead::has_key(self.storage, key)
            }
        }
    }

    fn iter_prefix(
        &'iter self,
        prefix: &namada_core::types::storage::Key,
    ) -> namada_core::ledger::storage_api::Result<Self::PrefixIter> {
        let write_log_iter = self.write_log.iter_prefix(prefix);
        let storage_iter = StorageRead::iter_prefix(self.storage, prefix);
        // TODO: change the PrefixIter type
        // TODO: maybe we can construct `storage_iter` as Peekable?
        Ok((write_log_iter, storage_iter))
    }

    fn iter_next(
        &self,
        iter: &mut Self::PrefixIter,
    ) -> namada_core::ledger::storage_api::Result<Option<(String, Vec<u8>)>>
    {
        let (write_log_iter, storage_iter) = iter;
        while let Some((key, val, iter_gas)) =
            storage_iter.clone().peekable().next()
        {
            // TODO: check if there's anything in write_log with a key that's LT
            // this `key` and if so, return it instead.
            // If not, we can call mutable `next()` on storage_iter to consume
            // it

            let (log_val, log_gas) = self
                .write_log
                .read(&Key::parse(key.clone()).into_storage_result()?);
            match log_val {
                Some(&write_log::StorageModification::Write { ref value }) => {
                    return Ok(Some((key, value.clone())));
                }
                Some(&write_log::StorageModification::Delete) => {
                    // check the next because the key has already deleted
                    continue;
                }
                Some(&write_log::StorageModification::InitAccount {
                    ..
                }) => {
                    // a VP of a new account doesn't need to be iterated
                    continue;
                }
                Some(&write_log::StorageModification::Temp { ref value }) => {
                    return Ok(Some((key, value.clone())));
                }
                None => return Ok(Some((key, val))),
            }
        }
        // If nothing is left in `storage_iter`, consume the rest of the
        // write_log_iter
        Ok(None)
    }

    fn get_chain_id(&self) -> namada_core::ledger::storage_api::Result<String> {
        StorageRead::get_chain_id(self.storage)
    }

    fn get_block_height(
        &self,
    ) -> namada_core::ledger::storage_api::Result<
        namada_core::types::storage::BlockHeight,
    > {
        StorageRead::get_block_height(self.storage)
    }

    fn get_block_hash(
        &self,
    ) -> namada_core::ledger::storage_api::Result<
        namada_core::types::storage::BlockHash,
    > {
        StorageRead::get_block_hash(self.storage)
    }

    fn get_block_epoch(
        &self,
    ) -> namada_core::ledger::storage_api::Result<
        namada_core::types::storage::Epoch,
    > {
        StorageRead::get_block_epoch(self.storage)
    }

    fn get_tx_index(
        &self,
    ) -> namada_core::ledger::storage_api::Result<
        namada_core::types::storage::TxIndex,
    > {
        StorageRead::get_tx_index(self.storage)
    }

    fn get_native_token(
        &self,
    ) -> namada_core::ledger::storage_api::Result<
        namada_core::types::address::Address,
    > {
        StorageRead::get_native_token(self.storage)
    }
}

impl<D, H> StorageWrite for StorageWithWriteLog<'_, D, H>
where
    D: DB + for<'iter> DBIter<'iter>,
    H: StorageHasher,
{
    fn write_bytes(
        &mut self,
        key: &namada_core::types::storage::Key,
        val: impl AsRef<[u8]>,
    ) -> namada_core::ledger::storage_api::Result<()> {
        let _ = self
            .write_log
            .write(key, val.as_ref().to_vec())
            .into_storage_result();
        Ok(())
    }

    fn delete(
        &mut self,
        key: &namada_core::types::storage::Key,
    ) -> namada_core::ledger::storage_api::Result<()> {
        let _ = self.write_log.delete(key).into_storage_result();
        Ok(())
    }
}
