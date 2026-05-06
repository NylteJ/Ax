#[macro_export]
macro_rules! impl_arc {
    ($struct_name:ident, $full:ident, $ops:ident, $ref_var: ident) => {
        pub struct $struct_name<Ops: ?Sized + $ops = dyn $ops> {
            ptr: NonNull<$full<Ops>>,
        }

        impl $struct_name {
            pub fn new<T: $ops + 'static>(val: $full<T>) -> Self {
                debug_assert_eq!(val.$ref_var.load(core::sync::atomic::Ordering::Relaxed), 1);
                Self {
                    ptr: NonNull::from_ref(alloc::boxed::Box::leak(alloc::boxed::Box::new(val))),
                }
            }

            pub fn addr_eq(&self, other: &Self) -> bool {
                self.ptr.addr() == other.ptr.addr()
            }

            pub fn addr(&self) -> usize {
                self.ptr.addr().into()
            }
        }

        impl<T: ?Sized + $ops + core::marker::Unsize<U>, U: ?Sized + $ops>
            core::ops::CoerceUnsized<$struct_name<U>> for $struct_name<T>
        {
        }

        impl<Ops: ?Sized + $ops> Clone for $struct_name<Ops> {
            fn clone(&self) -> Self {
                self.$ref_var
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                Self { ptr: self.ptr }
            }
        }

        impl<Ops: ?Sized + $ops> Drop for $struct_name<Ops> {
            fn drop(&mut self) {
                if self
                    .$ref_var
                    .fetch_sub(1, core::sync::atomic::Ordering::Release)
                    != 1
                {
                    return;
                }

                core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

                drop(unsafe { alloc::boxed::Box::from_raw(self.ptr.as_ptr()) });
            }
        }

        impl<Ops: ?Sized + $ops> Deref for $struct_name<Ops> {
            type Target = $full<Ops>;

            fn deref(&self) -> &Self::Target {
                unsafe { self.ptr.as_ref() }
            }
        }
    };
}
