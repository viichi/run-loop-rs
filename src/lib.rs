
#![feature(raw)]
#![feature(alloc_layout_extra)]
#![feature(deadline_api)]
#![feature(rc_into_raw_non_null)]

pub mod linked_list;
pub mod binary_heap;

mod run_loop;

pub use run_loop::*;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
