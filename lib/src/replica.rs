use crate::{result::TCResult, status::TCStatus, string::TCString, task::TCTask, uuid::TCUuid};
use std::cell::{RefCell, RefMut};
use taskchampion::{Replica, StorageConfig, Uuid};

/// A replica represents an instance of a user's task data, providing an easy interface
/// for querying and modifying that data.
///
/// TCReplicas are not threadsafe.
pub struct TCReplica {
    inner: RefCell<Replica>,
    error: Option<TCString<'static>>,
}

impl TCReplica {
    /// Borrow a TCReplica from C as an argument.
    ///
    /// # Safety
    ///
    /// The pointer must not be NULL.  It is the caller's responsibility to ensure that the
    /// lifetime assigned to the reference and the lifetime of the TCReplica itself do not outlive
    /// the lifetime promised by C.
    pub(crate) unsafe fn from_arg_ref<'a>(tcreplica: *mut TCReplica) -> &'a mut Self {
        debug_assert!(!tcreplica.is_null());
        &mut *tcreplica
    }

    /// Convert this to a return value for handing off to C.
    pub(crate) fn return_val(self) -> *mut TCReplica {
        Box::into_raw(Box::new(self))
    }
}

impl From<Replica> for TCReplica {
    fn from(rep: Replica) -> TCReplica {
        TCReplica {
            inner: RefCell::new(rep),
            error: None,
        }
    }
}

fn err_to_tcstring(e: impl std::string::ToString) -> TCString<'static> {
    TCString::from(e.to_string())
}

/// Utility function to allow using `?` notation to return an error value.  This makes
/// a mutable borrow, because most Replica methods require a `&mut`.
fn wrap<'a, T, F>(rep: *mut TCReplica, f: F, err_value: T) -> T
where
    F: FnOnce(RefMut<'_, Replica>) -> anyhow::Result<T>,
{
    // SAFETY:
    //  - rep is not null (promised by caller)
    //  - rep outlives 'a (promised by caller)
    let rep: &'a mut TCReplica = unsafe { TCReplica::from_arg_ref(rep) };
    rep.error = None;
    match f(rep.inner.borrow_mut()) {
        Ok(v) => v,
        Err(e) => {
            rep.error = Some(err_to_tcstring(e));
            err_value
        }
    }
}

/// Create a new TCReplica with an in-memory database.  The contents of the database will be
/// lost when it is freed.
#[no_mangle]
pub extern "C" fn tc_replica_new_in_memory() -> *mut TCReplica {
    let storage = StorageConfig::InMemory
        .into_storage()
        .expect("in-memory always succeeds");
    TCReplica::from(Replica::new(storage)).return_val()
}

/// Create a new TCReplica with an on-disk database having the given filename. The filename must
/// not be NULL. On error, a string is written to the `error_out` parameter (if it is not NULL) and
/// NULL is returned.
#[no_mangle]
pub extern "C" fn tc_replica_new_on_disk<'a>(
    path: *mut TCString,
    error_out: *mut *mut TCString,
) -> *mut TCReplica {
    // SAFETY:
    //  - tcstring is not NULL (promised by caller)
    //  - caller is exclusive owner of tcstring (implicitly promised by caller)
    let path = unsafe { TCString::from_arg(path) };
    let storage_res = StorageConfig::OnDisk {
        taskdb_dir: path.to_path_buf(),
    }
    .into_storage();

    let storage = match storage_res {
        Ok(storage) => storage,
        Err(e) => {
            if !error_out.is_null() {
                unsafe {
                    *error_out = err_to_tcstring(e).return_val();
                }
            }
            return std::ptr::null_mut();
        }
    };

    TCReplica::from(Replica::new(storage)).return_val()
}

// TODO: tc_replica_all_tasks
// TODO: tc_replica_all_task_uuids
// TODO: tc_replica_working_set

/// Get an existing task by its UUID.
///
/// Returns NULL when the task does not exist, and on error.  Consult tc_replica_error
/// to distinguish the two conditions.
#[no_mangle]
pub extern "C" fn tc_replica_get_task(rep: *mut TCReplica, uuid: TCUuid) -> *mut TCTask {
    wrap(
        rep,
        |mut rep| {
            let uuid: Uuid = uuid.into();
            if let Some(task) = rep.get_task(uuid)? {
                Ok(TCTask::from(task).return_val())
            } else {
                Ok(std::ptr::null_mut())
            }
        },
        std::ptr::null_mut(),
    )
}

/// Create a new task.  The task must not already exist.
///
/// The description must not be NULL.
///
/// Returns the task, or NULL on error.
#[no_mangle]
pub extern "C" fn tc_replica_new_task(
    rep: *mut TCReplica,
    status: TCStatus,
    description: *mut TCString,
) -> *mut TCTask {
    // SAFETY:
    //  - tcstring is not NULL (promised by caller)
    //  - caller is exclusive owner of tcstring (implicitly promised by caller)
    let description = unsafe { TCString::from_arg(description) };
    wrap(
        rep,
        |mut rep| {
            let task = rep.new_task(status.into(), description.as_str()?.to_string())?;
            Ok(TCTask::from(task).return_val())
        },
        std::ptr::null_mut(),
    )
}

/// Create a new task.  The task must not already exist.
///
/// Returns the task, or NULL on error.
#[no_mangle]
pub extern "C" fn tc_replica_import_task_with_uuid(
    rep: *mut TCReplica,
    uuid: TCUuid,
) -> *mut TCTask {
    wrap(
        rep,
        |mut rep| {
            let uuid: Uuid = uuid.into();
            let task = rep.import_task_with_uuid(uuid)?;
            Ok(TCTask::from(task).return_val())
        },
        std::ptr::null_mut(),
    )
}

// TODO: tc_replica_sync

/// Undo local operations until the most recent UndoPoint.
///
/// Returns TC_RESULT_TRUE if an undo occurred, TC_RESULT_FALSE if there are no operations
/// to be undone, or TC_RESULT_ERROR on error.
#[no_mangle]
pub extern "C" fn tc_replica_undo<'a>(rep: *mut TCReplica) -> TCResult {
    wrap(
        rep,
        |mut rep| {
            Ok(if rep.undo()? {
                TCResult::True
            } else {
                TCResult::False
            })
        },
        TCResult::Error,
    )
}

/// Get the latest error for a replica, or NULL if the last operation succeeded.  Subsequent calls
/// to this function will return NULL.  The rep pointer must not be NULL.  The caller must free the
/// returned string.
#[no_mangle]
pub extern "C" fn tc_replica_error<'a>(rep: *mut TCReplica) -> *mut TCString<'static> {
    // SAFETY:
    //  - rep is not null (promised by caller)
    //  - rep outlives 'a (promised by caller)
    let rep: &'a mut TCReplica = unsafe { TCReplica::from_arg_ref(rep) };
    if let Some(tcstring) = rep.error.take() {
        tcstring.return_val()
    } else {
        std::ptr::null_mut()
    }
}

/// Free a TCReplica.
#[no_mangle]
pub extern "C" fn tc_replica_free(rep: *mut TCReplica) {
    debug_assert!(!rep.is_null());
    drop(unsafe { Box::from_raw(rep) });
}

// TODO: tc_replica_rebuild_working_set
// TODO: tc_replica_add_undo_point
