// Copyright (c) 2004-present, Facebook, Inc.
// All Rights Reserved.
//
// This software may be used and distributed according to the terms of the
// GNU General Public License version 2 or any later version.

#![deny(warnings)]

#[macro_use]
extern crate failure_derive;
extern crate failure_ext as failure;
extern crate futures;
extern crate serde;
#[macro_use]
extern crate serde_derive;

extern crate mercurial_types;

use std::fmt;
use std::sync::Arc;

use futures::{Future, IntoFuture};
use futures::future::FutureResult;

use mercurial_types::{NodeHash, RepoPath};

mod errors {
    use super::*;

    pub use failure::{Error, Result, ResultExt};

    #[derive(Debug)]
    pub struct OptionNodeHash(pub Option<NodeHash>);

    impl fmt::Display for OptionNodeHash {
        fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
            match &self.0 {
                &Some(ref nodehash) => nodehash.fmt(fmt),
                &None => write!(fmt, "(unknown)"),
            }
        }
    }

    #[derive(Debug, Fail)]
    pub enum ErrorKind {
        #[fail(display = "linknode not found for {}, node {}", _0, _1)] NotFound(RepoPath, NodeHash),
        #[fail(display = "linknode already exists for {}, node {} (linknodes: existing {}, new {})",
               path, node, old_linknode, new_linknode)]
        AlreadyExists {
            path: RepoPath,
            node: NodeHash,
            old_linknode: OptionNodeHash,
            new_linknode: NodeHash,
        },
        #[fail(display = "linknode storage error")] StorageError,
    }
}

pub use errors::*;

/// Trait representing the interface to a linknodes store, which maps a path plus manifest or file
/// node hash to a changeset hash. At the moment this is a 1:1 mapping, but this will eventually
/// allow a 1:many mapping.
///
/// In principle, linknodes (especially 1:many) can be cached and regenerated. In practice,
/// Mercurial's storage and wire protocol is designed around storing linknodes as intrinsic data,
/// so Mononoke does the same.
pub trait Linknodes: Send + Sync + 'static {
    // Get will become a Stream once 1:many mappings are enabled.
    type Get: Future<Item = NodeHash, Error = Error> + Send + 'static;
    type Effect: Future<Item = (), Error = Error> + Send + 'static;

    fn add(&self, path: RepoPath, node: &NodeHash, linknode: &NodeHash) -> Self::Effect;
    fn get(&self, path: RepoPath, node: &NodeHash) -> Self::Get;
}

/// A linknodes implementation that never stores anything.
pub struct NoopLinknodes;

impl NoopLinknodes {
    #[inline]
    pub fn new() -> Self {
        NoopLinknodes
    }
}

impl Linknodes for NoopLinknodes {
    type Get = FutureResult<NodeHash, Error>;
    type Effect = FutureResult<(), Error>;

    #[inline]
    fn get(&self, path: RepoPath, node: &NodeHash) -> Self::Get {
        Err(ErrorKind::NotFound(path, *node).into()).into_future()
    }

    #[inline]
    fn add(&self, _path: RepoPath, _node: &NodeHash, _linknode: &NodeHash) -> Self::Effect {
        Ok(()).into_future()
    }
}

impl<L> Linknodes for Arc<L>
where
    L: Linknodes,
{
    type Get = L::Get;
    type Effect = L::Effect;

    #[inline]
    fn get(&self, path: RepoPath, node: &NodeHash) -> Self::Get {
        (**self).get(path, node)
    }

    #[inline]
    fn add(&self, path: RepoPath, node: &NodeHash, linknode: &NodeHash) -> Self::Effect {
        (**self).add(path, node, linknode)
    }
}

/// A struct representing all the data associated with a linknode. This definition is here so that
/// it can be shared across memory-based and file-based linknodes.
#[derive(Clone, Serialize, Deserialize)]
pub struct LinknodeData {
    pub path: RepoPath,
    pub node: NodeHash,
    pub linknode: NodeHash,
}
