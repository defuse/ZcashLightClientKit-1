use failure::format_err;
use ffi_helpers::panic::catch_panic;
use std::ffi::{CStr, CString, OsStr};
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::slice;
use zcash_client_backend::{
    constants::{testnet::HRP_SAPLING_EXTENDED_SPENDING_KEY},
    encoding::{decode_extended_spending_key, encode_extended_spending_key},
    keys::spending_key,
};
use zcash_client_sqlite::{
    address::RecipientAddress,
    chain::{rewind_to_height, validate_combined_chain},
    error::ErrorKind,
    init::{init_accounts_table, init_blocks_table, init_data_database},
    query::{
        get_address, get_balance, get_received_memo_as_utf8, get_sent_memo_as_utf8,
        get_verified_balance,
    },
    scan::scan_cached_blocks,
    transact::create_to_address,
};
use zcash_primitives::{
    block::BlockHash, note_encryption::Memo, transaction::components::Amount,
    zip32::ExtendedFullViewingKey,
};
use zcash_proofs::prover::LocalTxProver;

fn unwrap_exc_or<T>(exc: Result<T, ()>, def: T) -> T {
    match exc {
        Ok(value) => value,
        Err(_) => def,
    }
}

fn unwrap_exc_or_null<T>(exc: Result<T, ()>) -> T
where
    T: ffi_helpers::Nullable,
{
    match exc {
        Ok(value) => value,
        Err(_) => ffi_helpers::Nullable::NULL,
    }
}

/// Returns the length of the last error message to be logged.
#[no_mangle]
pub extern "C" fn zcashlc_last_error_length() -> i32 {
    ffi_helpers::error_handling::last_error_length()
}

/// Copies the last error message into the provided allocated buffer.
#[no_mangle]
pub unsafe extern "C" fn zcashlc_error_message_utf8(buf: *mut c_char, length: i32) -> i32 {
    ffi_helpers::error_handling::error_message_utf8(buf, length)
}

/// Clears the record of the last error message.
#[no_mangle]
pub extern "C" fn zcashlc_clear_last_error() {
    ffi_helpers::error_handling::clear_last_error()
}

/// Sets up the internal structure of the data database.
#[no_mangle]
pub extern "C" fn zcashlc_init_data_database(db_data: *const u8, db_data_len: usize) -> i32 {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));

        init_data_database(&db_data)
            .map(|()| 1)
            .map_err(|e| format_err!("Error while initializing data DB: {}", e))
    });
    unwrap_exc_or_null(res)
}

/// Initialises the data database with the given number of accounts using the given seed.
///
/// Returns the ExtendedSpendingKeys for the accounts. The caller should store these
/// securely for use while spending.
///
/// Call `zcashlc_vec_string_free` on the returned pointer when you are finished with it.
#[no_mangle]
pub extern "C" fn zcashlc_init_accounts_table(
    db_data: *const u8,
    db_data_len: usize,
    seed: *const u8,
    seed_len: usize,
    accounts: i32,
) -> *mut *mut c_char {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));
        let seed = unsafe { slice::from_raw_parts(seed, seed_len) };
        let accounts = if accounts >= 0 {
            accounts as u32
        } else {
            return Err(format_err!("accounts argument must be positive"));
        };

        let extsks: Vec<_> = (0..accounts)
            .map(|account| spending_key(&seed, 1, account))
            .collect();
        let extfvks: Vec<_> = extsks.iter().map(ExtendedFullViewingKey::from).collect();

        match init_accounts_table(&db_data, &extfvks) {
            Ok(()) => (),
            Err(e) => match e.kind() {
                ErrorKind::TableNotEmpty => {
                    // Ignore this error.
                }
                _ => return Err(format_err!("Error while initializing accounts: {}", e)),
            },
        }

        // Return the ExtendedSpendingKeys for the created accounts.
        let mut v: Vec<_> = extsks
            .iter()
            .map(|extsk| {
                let encoded =
                    encode_extended_spending_key(HRP_SAPLING_EXTENDED_SPENDING_KEY, extsk);
                CString::new(encoded).unwrap().into_raw()
            })
            .collect();
        assert!(v.len() == v.capacity());
        let p = v.as_mut_ptr();
        std::mem::forget(v);
        Ok(p)
    });
    unwrap_exc_or_null(res)
}

/// Initialises the data database with the given block.
///
/// This enables a newly-created database to be immediately-usable, without needing to
/// synchronise historic blocks.
#[no_mangle]
pub extern "C" fn zcashlc_init_blocks_table(
    db_data: *const u8,
    db_data_len: usize,
    height: i32,
    hash_hex: *const c_char,
    time: u32,
    sapling_tree_hex: *const c_char,
) -> i32 {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));
        let hash = {
            let mut hash = hex::decode(unsafe { CStr::from_ptr(hash_hex) }.to_str()?).unwrap();
            hash.reverse();
            BlockHash::from_slice(&hash)
        };
        let sapling_tree =
            hex::decode(unsafe { CStr::from_ptr(sapling_tree_hex) }.to_str()?).unwrap();

        match init_blocks_table(&db_data, height, hash, time, &sapling_tree) {
            Ok(()) => Ok(1),
            Err(e) => Err(format_err!("Error while initializing blocks table: {}", e)),
        }
    });
    unwrap_exc_or_null(res)
}

/// Returns the address for the account.
///
/// Call `zcashlc_string_free` on the returned pointer when you are finished with it.
#[no_mangle]
pub extern "C" fn zcashlc_get_address(
    db_data: *const u8,
    db_data_len: usize,
    account: i32,
) -> *mut c_char {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));
        let account = if account >= 0 {
            account as u32
        } else {
            return Err(format_err!("accounts argument must be positive"));
        };

        match get_address(&db_data, account) {
            Ok(addr) => {
                let c_str_addr = CString::new(addr).unwrap();
                Ok(c_str_addr.into_raw())
            }
            Err(e) => Err(format_err!("Error while fetching address: {}", e)),
        }
    });
    unwrap_exc_or_null(res)
}

/// Returns the balance for the account, including all unspent notes that we know about.
#[no_mangle]
pub extern "C" fn zcashlc_get_balance(db_data: *const u8, db_data_len: usize, account: i32) -> i64 {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));
        let account = if account >= 0 {
            account as u32
        } else {
            return Err(format_err!("account argument must be positive"));
        };

        match get_balance(&db_data, account) {
            Ok(balance) => Ok(balance.into()),
            Err(e) => Err(format_err!("Error while fetching balance: {}", e)),
        }
    });
    unwrap_exc_or(res, -1)
}

/// Returns the verified balance for the account, which ignores notes that have been
/// received too recently and are not yet deemed spendable.
#[no_mangle]
pub extern "C" fn zcashlc_get_verified_balance(
    db_data: *const u8,
    db_data_len: usize,
    account: i32,
) -> i64 {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));
        let account = if account >= 0 {
            account as u32
        } else {
            return Err(format_err!("account argument must be positive"));
        };

        match get_verified_balance(&db_data, account) {
            Ok(balance) => Ok(balance.into()),
            Err(e) => Err(format_err!("Error while fetching verified balance: {}", e)),
        }
    });
    unwrap_exc_or(res, -1)
}

/// Returns the memo for a received note, if it is known and a valid UTF-8 string.
///
/// The note is identified by its row index in the `received_notes` table within the data
/// database.
///
/// Call `zcashlc_string_free` on the returned pointer when you are finished with it.
#[no_mangle]
pub extern "C" fn zcashlc_get_received_memo_as_utf8(
    db_data: *const u8,
    db_data_len: usize,
    id_note: i64,
) -> *mut c_char {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));

        let memo = match get_received_memo_as_utf8(db_data, id_note) {
            Ok(memo) => memo.unwrap_or_default(),
            Err(e) => return Err(format_err!("Error while fetching memo: {}", e)),
        };

        Ok(CString::new(memo).unwrap().into_raw())
    });
    unwrap_exc_or_null(res)
}

/// Returns the memo for a sent note, if it is known and a valid UTF-8 string.
///
/// The note is identified by its row index in the `sent_notes` table within the data
/// database.
///
/// Call `zcashlc_string_free` on the returned pointer when you are finished with it.
#[no_mangle]
pub extern "C" fn zcashlc_get_sent_memo_as_utf8(
    db_data: *const u8,
    db_data_len: usize,
    id_note: i64,
) -> *mut c_char {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));

        let memo = match get_sent_memo_as_utf8(db_data, id_note) {
            Ok(memo) => memo.unwrap_or_default(),
            Err(e) => return Err(format_err!("Error while fetching memo: {}", e)),
        };

        Ok(CString::new(memo).unwrap().into_raw())
    });
    unwrap_exc_or_null(res)
}

/// Checks that the scanned blocks in the data database, when combined with the recent
/// `CompactBlock`s in the cache database, form a valid chain.
///
/// This function is built on the core assumption that the information provided in the
/// cache database is more likely to be accurate than the previously-scanned information.
/// This follows from the design (and trust) assumption that the `lightwalletd` server
/// provides accurate block information as of the time it was requested.
///
/// Returns:
/// - `-1` if the combined chain is valid.
/// - `upper_bound` if the combined chain is invalid.
///   `upper_bound` is the height of the highest invalid block (on the assumption that the
///   highest block in the cache database is correct).
/// - `0` if there was an error during validation unrelated to chain validity.
///
/// This function does not mutate either of the databases.
#[no_mangle]
pub extern "C" fn zcashlc_validate_combined_chain(
    db_cache: *const u8,
    db_cache_len: usize,
    db_data: *const u8,
    db_data_len: usize,
) -> i32 {
    let res = catch_panic(|| {
        let db_cache = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_cache, db_cache_len)
        }));
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));

        if let Err(e) = validate_combined_chain(&db_cache, &db_data) {
            match e.kind() {
                ErrorKind::InvalidChain(upper_bound, _) => Ok(*upper_bound),
                _ => Err(format_err!("Error while validating chain: {}", e)),
            }
        } else {
            // All blocks are valid, so "highest invalid block height" is below genesis.
            Ok(-1)
        }
    });
    unwrap_exc_or_null(res)
}

/// Rewinds the data database to the given height.
///
/// If the requested height is greater than or equal to the height of the last scanned
/// block, this function does nothing.
#[no_mangle]
pub extern "C" fn zcashlc_rewind_to_height(
    db_data: *const u8,
    db_data_len: usize,
    height: i32,
) -> i32 {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));

        match rewind_to_height(&db_data, height) {
            Ok(()) => Ok(1),
            Err(e) => Err(format_err!(
                "Error while rewinding data DB to height {}: {}",
                height,
                e
            )),
        }
    });
    unwrap_exc_or_null(res)
}

/// Scans new blocks added to the cache for any transactions received by the tracked
/// accounts.
///
/// This function pays attention only to cached blocks with heights greater than the
/// highest scanned block in `db_data`. Cached blocks with lower heights are not verified
/// against previously-scanned blocks. In particular, this function **assumes** that the
/// caller is handling rollbacks.
///
/// For brand-new light client databases, this function starts scanning from the Sapling
/// activation height. This height can be fast-forwarded to a more recent block by calling
/// [`zcashlc_init_blocks_table`] before this function.
///
/// Scanned blocks are required to be height-sequential. If a block is missing from the
/// cache, an error will be signalled.
#[no_mangle]
pub extern "C" fn zcashlc_scan_blocks(
    db_cache: *const u8,
    db_cache_len: usize,
    db_data: *const u8,
    db_data_len: usize,
) -> i32 {
    let res = catch_panic(|| {
        let db_cache = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_cache, db_cache_len)
        }));
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));

        match scan_cached_blocks(&db_cache, &db_data) {
            Ok(()) => Ok(1),
            Err(e) => Err(format_err!("Error while scanning blocks: {}", e)),
        }
    });
    unwrap_exc_or_null(res)
}

/// Creates a transaction paying the specified address from the given account.
///
/// Returns the row index of the newly-created transaction in the `transactions` table
/// within the data database. The caller can read the raw transaction bytes from the `raw`
/// column in order to broadcast the transaction to the network.
///
/// Do not call this multiple times in parallel, or you will generate transactions that
/// double-spend the same notes.
#[no_mangle]
pub extern "C" fn zcashlc_create_to_address(
    db_data: *const u8,
    db_data_len: usize,
    account: i32,
    extsk: *const c_char,
    to: *const c_char,
    value: i64,
    memo: *const c_char,
    spend_params: *const u8,
    spend_params_len: usize,
    output_params: *const u8,
    output_params_len: usize,
) -> i64 {
    let res = catch_panic(|| {
        let db_data = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(db_data, db_data_len)
        }));
        let account = if account >= 0 {
            account as u32
        } else {
            return Err(format_err!("account argument must be positive"));
        };
        let extsk = unsafe { CStr::from_ptr(extsk) }.to_str()?;
        let to = unsafe { CStr::from_ptr(to) }.to_str()?;
        let value =
            Amount::from_i64(value).map_err(|()| format_err!("Invalid amount, out of range"))?;
        if value.is_negative() {
            return Err(format_err!("Amount is negative"));
        }
        let memo = unsafe { CStr::from_ptr(memo) }.to_str()?;
        let spend_params = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(spend_params, spend_params_len)
        }));
        let output_params = Path::new(OsStr::from_bytes(unsafe {
            slice::from_raw_parts(output_params, output_params_len)
        }));

        let extsk = match decode_extended_spending_key(HRP_SAPLING_EXTENDED_SPENDING_KEY, &extsk) {
            Ok(Some(extsk)) => extsk,
            Ok(None) => {
                return Err(format_err!("ExtendedSpendingKey is for the wrong network"));
            }
            Err(e) => {
                return Err(format_err!("Invalid ExtendedSpendingKey: {}", e));
            }
        };

        let to = match RecipientAddress::from_str(&to) {
            Some(to) => to,
            None => {
                return Err(format_err!("PaymentAddress is for the wrong network"));
            }
        };

        let memo = Memo::from_str(&memo);

        let prover = LocalTxProver::new(spend_params, output_params);
        
        create_to_address(
            &db_data,
            0x2bb4_0e60, // BLOSSOM_CONSENSUS_BRANCH_ID
            prover,
            (account, &extsk),
            &to,
            value,
            memo,
        )
        .map_err(|e| format_err!("Error while sending funds: {}", e))
    });
    unwrap_exc_or(res, -1)
}

/// Frees strings returned by other zcashlc functions.
#[no_mangle]
pub extern "C" fn zcashlc_string_free(s: *mut c_char) {
    unsafe {
        if s.is_null() {
            return;
        }
        CString::from_raw(s)
    };
}

/// Frees vectors of strings returned by other zcashlc functions.
#[no_mangle]
pub extern "C" fn zcashlc_vec_string_free(v: *mut *mut c_char, len: usize) {
    unsafe {
        if v.is_null() {
            return;
        }
        // All Vecs created by other functions MUST have length == capacity.
        let v = Vec::from_raw_parts(v, len, len);
        v.into_iter().map(|s| CString::from_raw(s)).for_each(drop);
    };
}
