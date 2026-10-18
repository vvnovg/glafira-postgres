//! Блокировки без паники на отравлении. Если поток упал с паникой под блокировкой, остальные
//! продолжают работать с последним записанным значением: во всём проекте значение под
//! блокировкой заменяется целиком, поэтому оно остаётся согласованным.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub trait MutexExt<T> {
    fn lock_safe(&self) -> MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn lock_safe(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub trait RwLockExt<T> {
    fn read_safe(&self) -> RwLockReadGuard<'_, T>;
    fn write_safe(&self) -> RwLockWriteGuard<'_, T>;
}

impl<T> RwLockExt<T> for RwLock<T> {
    fn read_safe(&self) -> RwLockReadGuard<'_, T> {
        self.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write_safe(&self) -> RwLockWriteGuard<'_, T> {
        self.write().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, RwLock};

    #[test]
    fn poisoned_locks_are_still_usable() {
        let m = Arc::new(Mutex::new(1));
        let rw = Arc::new(RwLock::new(1));
        let (m2, rw2) = (m.clone(), rw.clone());
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            let _w = rw2.write().unwrap();
            panic!("отравляем блокировки");
        })
        .join();
        assert!(m.is_poisoned() && rw.is_poisoned());
        *m.lock_safe() += 1;
        *rw.write_safe() += 1;
        assert_eq!(*m.lock_safe(), 2);
        assert_eq!(*rw.read_safe(), 2);
    }
}
