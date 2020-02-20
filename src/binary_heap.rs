
use std::ptr::NonNull;
use std::usize;
use std::mem;
use std::cmp::Ordering;
use std::ops::{Deref, DerefMut};

pub struct Node<K, V: ?Sized> {
    index: usize,
    key: K,
    value: V,
}

pub type NodePtr<K, V> = NonNull<Node<K, V>>;

impl<K, V> Node<K, V> {
    pub fn new(key: K, value: V) -> Node<K, V> {
        Node {
            index: usize::MAX,
            key,
            value,
        }
    }
}

impl<K, V: ?Sized> Node<K, V> {
    pub fn index(this: &Self) -> usize { this.index }
    pub fn key(this: &Self) -> &K { &this.key }
    pub fn set_key(this: &mut Self, key: K) -> Option<K> {
        if this.index == usize::MAX {
            this.key = key;
            None
        } else {
            Some(key)
        }
    }
    pub unsafe fn key_mut(this: &mut Self) -> &mut K {
        &mut this.key
    }
}

impl<K: Clone, V: Clone> Clone for Node<K, V> {
    fn clone(&self) -> Node<K, V> {
        Node {
            index: usize::MAX,
            key: self.key.clone(),
            value: self.value.clone(),
        }
    }
}

impl<K, V: ?Sized> Deref for Node<K, V> {
    type Target = V;
    fn deref(&self) -> &V {
        &self.value
    }
}

impl<K, V: ?Sized> DerefMut for Node<K, V> {
    fn deref_mut(&mut self) -> &mut V {
        &mut self.value
    }
}

impl<K, V: ?Sized> AsRef<V> for Node<K, V> {
    fn as_ref(&self) -> &V {
        &self.value
    }
}

impl<K, V: ?Sized> AsMut<V> for Node<K, V> {
    fn as_mut(&mut self) -> &mut V {
        &mut self.value
    }
}


pub struct BinaryHeap<K, V: ?Sized> {
    data: Vec<NodePtr<K, V>>,
}

impl<K, V: ?Sized> Default for BinaryHeap<K, V> {
    fn default() -> Self {
        BinaryHeap { data: Vec::default() }
    }
}

impl<K, V: ?Sized> BinaryHeap<K, V> {
    pub fn new() -> BinaryHeap<K, V> {
        Self::default()
    }
    pub fn with_capacity(capacity: usize) -> BinaryHeap<K, V> {
        BinaryHeap {
            data: Vec::with_capacity(capacity),
        }
    }
    pub fn into_vec(self) -> Vec<NodePtr<K, V>> {
        self.data
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn clear<F: FnMut(NodePtr<K, V>)>(&mut self, mut f: F) {
        for i in self.data.iter() {
            f(*i);
        }
        self.data.clear();
    }
}

impl<K: Unpin, V: Unpin + ?Sized> Unpin for Node<K, V> {}

impl<K: Ord, V: ?Sized> BinaryHeap<K, V> {
    pub fn peek(&self) -> Option<&NodePtr<K, V>> {
        self.data.get(0)
    }
    pub unsafe fn push(&mut self, mut item: NodePtr<K, V>) {
        item.as_mut().index = self.data.len();
        self.data.push(item);
        self.sift_up(0, item.as_ref().index);
    }
    pub unsafe fn pop(&mut self) -> Option<NodePtr<K, V>> {
        self.data.pop().map(|mut item| {
            if !self.is_empty() {
                item.as_mut().index = 0;
                mem::swap(&mut item, self.data.get_unchecked_mut(0));
                self.sift_down_to_bottom(0);
            }
            item.as_mut().index = usize::MAX;
            item
        })
    }
    pub unsafe fn update(&mut self, mut item: NodePtr<K, V>, key: K) {
        match key.cmp(&item.as_ref().key) {
            Ordering::Less => {
                item.as_mut().key = key;
                self.sift_up(0, item.as_ref().index);
            },
            Ordering::Greater => {
                item.as_mut().key = key;
                self.sift_down(item.as_ref().index);
            },
            _ => {},
        }
    }
    pub unsafe fn remove(&mut self, index: usize) -> NodePtr<K, V> {
        self.data.pop().map(|mut item| {
            if !self.is_empty() {
                item.as_mut().index = index;
                mem::swap(&mut item, self.data.get_unchecked_mut(index));
                self.sift_down_to_bottom(index);
            }
            item.as_mut().index = usize::MAX;
            item
        }).unwrap()
    }

    unsafe fn sift_up(&mut self, start: usize, pos: usize) -> usize {
        let mut hole = Hole::new(&mut self.data, pos);
        while hole.pos() > start {
            let parent = (hole.pos() - 1) / 2;
            if hole.key() >= hole.get_key(parent) {
                break;
            }
            hole.move_to(parent);
        }
        hole.pos()
    }

    unsafe fn sift_down(&mut self, pos: usize) -> usize {
        let end = self.len();
        let mut hole = Hole::new(&mut self.data, pos);
        let mut child = 2 * pos + 1;
        while child < end {
            let right = child + 1;
            if right < end && hole.get_key(right) < hole.get_key(child) {
                child = right;
            }
            if hole.key() <= hole.get_key(child) {
                break;
            }
            hole.move_to(child);
            child = 2 * hole.pos() + 1;
        }
        hole.pos()
    }

    unsafe fn sift_down_to_bottom(&mut self, pos: usize) -> usize {
        let end = self.len();
        let start = pos;

        let mut hole = Hole::new(&mut self.data, pos);
        let mut child = 2 * pos + 1;
        while child < end {
            let right = child + 1;
            if right < end && hole.get_key(right) < hole.get_key(child) {
                child = right;
            }
            hole.move_to(child);
            child = 2 * hole.pos() + 1;
        }

        while hole.pos() > start {
            let parent = (hole.pos() - 1) / 2;
            if hole.key() >= hole.get_key(parent) {
                break;
            }
            hole.move_to(parent);
        }
        hole.pos()
    }
}

struct Hole<'a, K, V: ?Sized> {
    data: &'a mut [NodePtr<K, V>],
    item: NodePtr<K, V>,
}

impl<'a, K, V: ?Sized> Hole<'a, K, V> {
    unsafe fn new(data: &'a mut[NodePtr<K, V>], pos: usize) -> Self {
        let item = *data.get_unchecked(pos);
        Hole {
            data,
            item,
        }
    }
    unsafe fn pos(&self) -> usize {
        self.item.as_ref().index
    }
    unsafe fn key(&self) -> &K {
        &self.item.as_ref().key
    }
    unsafe fn get_key(&self, index: usize) -> &K {
        &self.data.get_unchecked(index).as_ref().key
    }
    unsafe fn move_to(&mut self, index: usize) {
        let index_mut = self.data.get_unchecked_mut(self.pos()) as *mut _;
        *index_mut = *self.data.get_unchecked(index);
        (*index_mut).as_mut().index = self.pos();
        self.item.as_mut().index = index;
    }
}

impl<'a, K, V: ?Sized> Drop for Hole<'a, K, V> {
    fn drop(&mut self) {
        unsafe { *self.data.get_unchecked_mut(self.item.as_ref().index) = self.item; }
    }
}