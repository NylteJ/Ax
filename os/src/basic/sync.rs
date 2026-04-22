use core::cell::RefCell;
use core::ops::Deref;
use core::ops::DerefMut;

#[derive(Debug)]
pub struct RwLock<T> {
    value: RefCell<T>,
}

impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            value: RefCell::new(value),
        }
    }

    pub fn read(&self) -> impl Deref<Target = T> {
        self.value.borrow()
    }

    pub fn write(&self) -> impl DerefMut<Target = T> {
        self.value.borrow_mut()
    }
}

// SAFETY: 目前我们不支持并行，故而无需担心数据竞争
unsafe impl<T> Sync for RwLock<T> {}
unsafe impl<T> Send for RwLock<T> {}

pub struct Mutex<T> {
    value: RefCell<T>,
}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            value: RefCell::new(value),
        }
    }

    pub fn lock(&self) -> impl DerefMut<Target = T> {
        self.value.borrow_mut()
    }
}

// SAFETY: 目前我们不支持并行，故而无需担心数据竞争
unsafe impl<T> Sync for Mutex<T> {}
unsafe impl<T> Send for Mutex<T> {}
