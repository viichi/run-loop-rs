
use std::raw::TraitObject;
use std::ptr::{self, NonNull};
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};
use std::marker::Unpin;

pub struct Node<T: ?Sized> {
    next: TraitObject,
    prev: TraitObject,
    data: T,
}

pub type NodePtr<T> = NonNull<Node<T>>;

impl<T> Node<T> {
    pub fn new(data: T) -> Node<T> {
        unsafe {
            Node {
                next: MaybeUninit::zeroed().assume_init(),
                prev: MaybeUninit::zeroed().assume_init(),
                data,
            }
        }
    }
}

impl<T: ?Sized> Node<T> {
    fn next(&self) -> *mut Node<T> {
        unsafe { ptr::read(&self.next as *const _ as *const _) }
    }
    fn prev(&self) -> *mut Node<T> {
        unsafe { ptr::read(&self.prev as *const _ as *const _) }
    }
    fn set_next(&mut self, ptr: *mut Node<T>) {
        unsafe { ptr::write(&mut self.next as *mut _ as *mut _, ptr)}
    }
    fn set_prev(&mut self, ptr: *mut Node<T>) {
        unsafe { ptr::write(&mut self.prev as *mut _ as *mut _, ptr) }
    }
    fn set_next_null(&mut self) {
        set_null(&mut self.next);
    }
    fn set_prev_null(&mut self) {
        set_null(&mut self.prev);
    }
    pub fn get_ptr_from_raw(data: *const T) -> *const Node<T> {
        let data_offset = data_offset(data);
        let mut ptr = data as *const Node<T>;
        unsafe { ptr::write(&mut ptr as *mut _ as *mut *mut u8, (data as *mut u8).offset(-data_offset)) };
        ptr
    }
}

fn set_null<T: ?Sized>(ptr: &mut T) {
    unsafe { ptr::write(ptr as *mut _ as *mut usize, 0usize) }
}
fn data_offset<T: ?Sized>(ptr: *const T) -> isize {
    let layout = std::alloc::Layout::new::<Node<()>>();
    (layout.size() + layout.padding_needed_for(std::mem::align_of_val(&ptr))) as isize
}

impl<T: ?Sized> AsRef<T> for Node<T> {
    fn as_ref(&self) -> &T {
        &self.data
    }
}

impl<T: ?Sized> AsMut<T> for Node<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T: ?Sized> Deref for Node<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}

impl<T: ?Sized> DerefMut for Node<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T: ?Sized + Unpin> Unpin for Node<T> {}

pub struct List<T: ?Sized> {
    head: *mut Node<T>,
    tail: *mut Node<T>,
}

impl<T: ?Sized> Default for List<T> {
    fn default() -> List<T> {
        unsafe { MaybeUninit::zeroed().assume_init() }
    }
}

impl<T: ?Sized> List<T> {

    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    pub unsafe fn push_front(&mut self, node: NodePtr<T>) {
        let node = node.as_ptr();
        if self.head.is_null() {
            self.head = node;
            self.tail = node;
        } else {
            (*node).set_next(self.head);
            (*self.head).set_prev(node);
            self.head = node;
        }
    }

    pub unsafe fn push_back(&mut self, node: NodePtr<T>) {
        let node = node.as_ptr();
        if self.tail.is_null() {
            self.tail = node;
            self.head = node;
        } else {
            (*node).set_prev(self.tail);
            (*self.tail).set_next(node);
            self.tail = node;
        }
    }

    pub unsafe fn pop_front(&mut self) -> *mut Node<T> {
        let node = self.head;
        if !node.is_null() {
            let next = (*node).next();
            if !next.is_null() {
                self.head = next;
                (*node).set_next_null();
                (*next).set_prev_null();
            } else {
                set_null(&mut self.head);
                set_null(&mut self.tail);
            }
        }
        node
    }

    pub unsafe fn remove(&mut self, mut node: NodePtr<T>) {
        let next = node.as_mut().next();
        let prev = node.as_mut().prev();
        if prev.is_null() {
            self.head = next;
        } else {
            node.as_mut().set_prev_null();
            (*prev).set_next(next);
        }
        if next.is_null() {
            self.tail = prev;
        } else {
            node.as_mut().set_next_null();
            (*next).set_prev(prev);
        }
    }

    pub unsafe fn clear<F: FnMut(NodePtr<T>)>(&mut self, mut f: F) {
        let mut node = self.head;
        if !node.is_null() {
            set_null(&mut self.head);
            set_null(&mut self.tail);
            loop {
                let t = NonNull::new_unchecked(node);
                node = (*node).next();
                f(t);
                if node.is_null() {
                    break;
                }
            }
        }
    }

    /*
    pub fn drain(&mut self) -> *mut Node<T> {
        let node = self.head;
        unsafe {
            self.head = mem::zeroed();
            self.tail = mem::zeroed();
        }
        node
    }
    */
}