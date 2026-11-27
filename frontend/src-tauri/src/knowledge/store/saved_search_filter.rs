//! Connection-local SQLite predicate for the same visible text used in excerpts.

use super::{Arc, Mutex, SavedFilter};
use crate::knowledge::types::KnowledgeError;
use libsqlite3_sys as sqlite;
use std::{ffi::c_void, panic::catch_unwind, ptr, slice, str, time::Instant};

pub(super) async fn install(
    conn: &mut sqlx::SqliteConnection,
    filter: Option<Arc<Mutex<SavedFilter>>>,
) -> Result<(), KnowledgeError> {
    let mut handle = conn.lock_handle().await?;
    let registered = filter.is_some();
    let userdata = filter.map_or(ptr::null_mut(), |filter| {
        Arc::into_raw(filter).cast_mut().cast()
    });
    // SAFETY: SQLx excludes the connection worker while this handle is locked.
    // SQLite owns the Arc reference after registration, including on failure,
    // and calls the destructor on replacement, removal, or connection close.
    let status = unsafe {
        sqlite::sqlite3_create_function_v2(
            handle.as_raw_handle().as_ptr(),
            c"clawscribe_saved_match".as_ptr(),
            4,
            sqlite::SQLITE_UTF8 | sqlite::SQLITE_DIRECTONLY,
            userdata,
            registered.then_some(matches),
            None,
            None,
            registered.then_some(release),
        )
    };
    if status != sqlite::SQLITE_OK {
        return Err(KnowledgeError::Storage);
    }
    Ok(())
}

unsafe extern "C" fn release(userdata: *mut c_void) {
    // SAFETY: SQLite returns the reference transferred by install, once.
    drop(unsafe { Arc::from_raw(userdata.cast::<Mutex<SavedFilter>>()) });
}

unsafe extern "C" fn matches(
    context: *mut sqlite::sqlite3_context,
    count: i32,
    values: *mut *mut sqlite::sqlite3_value,
) {
    // SAFETY: SQLite invokes this callback with four live arguments and the
    // Arc supplied by install. No reference escapes the callback.
    let matched = catch_unwind(|| unsafe {
        if count != 4 {
            return Err(());
        }
        let args = slice::from_raw_parts(values, count as usize);
        let filter = sqlite::sqlite3_user_data(context).cast::<Mutex<SavedFilter>>();
        let mut filter = (*filter).lock().map_err(|_| ())?;
        if Instant::now() >= filter.deadline {
            // Let the next row through as a continuation marker. The SQL
            // LIMIT bounds this work even if SQLite keeps reading ahead.
            filter
                .first_unsearched
                .get_or_insert(sqlite::sqlite3_value_int64(args[0]));
            return Ok(true);
        }
        if sqlite::sqlite3_value_type(args[2]) == sqlite::SQLITE_NULL {
            filter.oversized = true;
            return Ok(false);
        }
        let Some(raw) = text(&args[2]) else {
            return Ok(false);
        };
        let blocks = sqlite::sqlite3_value_int(args[3]) != 0;
        let matched = filter
            .source
            .text(raw, blocks)
            .is_some_and(|body| filter.query.matches(&body));
        if matched {
            if let Some(seen) = filter.seen.as_mut() {
                return Ok(text(&args[1]).is_some_and(|id| seen.insert(id.to_owned())));
            }
        }
        Ok(matched)
    });
    // SAFETY: The callback context is still active. Catch panics at the C boundary.
    unsafe {
        match matched {
            Ok(Ok(matched)) => sqlite::sqlite3_result_int(context, i32::from(matched)),
            _ => sqlite::sqlite3_result_error_code(context, sqlite::SQLITE_ERROR),
        }
    }
}

// SAFETY: The argument must be valid for the callback and not coerced or
// otherwise modified while the resulting reference is in use.
unsafe fn text(value: &*mut sqlite::sqlite3_value) -> Option<&str> {
    let data = unsafe { sqlite::sqlite3_value_text(*value) };
    if data.is_null() {
        return None;
    }
    let len = unsafe { sqlite::sqlite3_value_bytes(*value) } as usize;
    str::from_utf8(unsafe { slice::from_raw_parts(data, len) }).ok()
}
