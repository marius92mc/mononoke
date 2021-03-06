// Copyright (c) 2004-present, Facebook, Inc.
// All Rights Reserved.
//
// This software may be used and distributed according to the terms of the
// GNU General Public License version 2 or any later version.

#![deny(warnings)]

extern crate ascii;
#[macro_use]
#[cfg(test)]
extern crate assert_matches;
#[macro_use]
extern crate failure_derive;
extern crate failure_ext as failure;
extern crate futures;
extern crate futures_ext;

extern crate bookmarks;
extern crate mercurial_types;
#[cfg(test)]
extern crate mercurial_types_mocks;
extern crate storage_types;

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::PathBuf;

use ascii::AsciiStr;
use failure::{Error, Result, ResultExt};
use futures::future;
use futures::stream::{self, Stream};
use futures_ext::{BoxFuture, BoxStream, StreamExt};

use bookmarks::Bookmarks;
use mercurial_types::NodeHash;
use storage_types::Version;

#[derive(Debug, Fail)]
pub enum ErrorKind {
    #[fail(display = "invalid bookmarks line: {}", _0)] InvalidBookmarkLine(String),
    #[fail(display = "invalid hash: {}", _0)] InvalidHash(String),
}

/// Implementation of bookmarks as they exist in stock Mercurial inside `.hg/bookmarks`.
/// The file has a list of entries:
///
/// ```
/// <hash1> <bookmark1-name>
/// <hash2> <bookmark2-name>
/// ...
/// ```
///
/// Bookmark names are arbitrary bytestrings, and hashes are always NodeHashes.
///
/// This implementation is read-only -- implementing write support would require interacting with
/// the locking mechanism Mercurial uses, and generally seems like it wouldn't be very useful.
#[derive(Clone, Debug)]
pub struct StockBookmarks {
    bookmarks: HashMap<Vec<u8>, NodeHash>,
}

impl StockBookmarks {
    pub fn read<P: Into<PathBuf>>(base: P) -> Result<Self> {
        let base = base.into();

        let file = fs::File::open(base.join("bookmarks"));
        match file {
            Ok(file) => Self::from_reader(file),
            Err(ref err) if err.kind() == io::ErrorKind::NotFound => {
                // The .hg/bookmarks file is not guaranteed to exist. Treat it is empty if it
                // doesn't.
                Ok(StockBookmarks {
                    bookmarks: HashMap::new(),
                })
            }
            Err(err) => Err(err.into()),
        }
    }

    fn from_reader<R: Read>(reader: R) -> Result<Self> {
        let mut bookmarks = HashMap::new();

        // Bookmark names might not be valid UTF-8, so use split() instead of lines().
        for line in BufReader::new(reader).split(b'\n') {
            let line = line?;
            // <hash><space><bookmark name>, where hash is 40 bytes, the space is 1 byte
            // and the bookmark name is at least 1 byte.
            if line.len() < 42 || line[40] != b' ' {
                return Err(
                    ErrorKind::InvalidBookmarkLine(
                        String::from_utf8_lossy(line.as_ref()).into_owned(),
                    ).into(),
                );
            }
            let bmname = &line[41..];
            let hash_slice = &line[..40];
            let hash = AsciiStr::from_ascii(&hash_slice).context(ErrorKind::InvalidHash(
                String::from_utf8_lossy(hash_slice).into_owned(),
            ))?;
            bookmarks.insert(
                bmname.into(),
                NodeHash::from_ascii_str(hash).context(ErrorKind::InvalidHash(
                    String::from_utf8_lossy(hash_slice).into_owned(),
                ))?,
            );
        }

        Ok(StockBookmarks { bookmarks })
    }
}

impl Bookmarks for StockBookmarks {
    fn get(&self, name: &AsRef<[u8]>) -> BoxFuture<Option<(NodeHash, Version)>, Error> {
        let value = match self.bookmarks.get(name.as_ref()) {
            Some(hash) => Some((*hash, Version::from(1))),
            None => None,
        };
        Box::new(future::result(Ok(value)))
    }

    fn keys(&self) -> BoxStream<Vec<u8>, Error> {
        // collect forces evaluation early, so that the stream can safely outlive self
        stream::iter_ok(
            self.bookmarks
                .keys()
                .map(|k| Ok(k.to_vec()))
                .collect::<Vec<_>>(),
        ).and_then(|x| x)
            .boxify()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use failure::Context;
    use futures::Future;
    use mercurial_types_mocks::nodehash;

    use super::*;

    fn assert_bookmark_get(
        bookmarks: &StockBookmarks,
        key: &AsRef<[u8]>,
        expected: Option<NodeHash>,
    ) {
        let expected = match expected {
            Some(hash) => Some((hash, Version::from(1))),
            None => None,
        };
        assert_eq!(bookmarks.get(key).wait().unwrap(), expected);
    }

    #[test]
    fn test_parse() {
        let disk_bookmarks = b"\
            1111111111111111111111111111111111111111 abc\n\
            2222222222222222222222222222222222222222 def\n\
            1111111111111111111111111111111111111111 test123\n";
        let reader = Cursor::new(&disk_bookmarks[..]);

        let bookmarks = StockBookmarks::from_reader(reader).unwrap();
        assert_bookmark_get(&bookmarks, &"abc", Some(nodehash::ONES_HASH));
        assert_bookmark_get(&bookmarks, &"def", Some(nodehash::TWOS_HASH));
        assert_bookmark_get(&bookmarks, &"test123", Some(nodehash::ONES_HASH));

        // Bookmarks that aren't present
        assert_bookmark_get(&bookmarks, &"abcdef", None);

        // keys should return all the keys here
        let mut list = bookmarks.keys().collect().wait().unwrap();
        list.sort();
        assert_eq!(list, vec![&b"abc"[..], &b"def"[..], &b"test123"[..]]);
    }

    /// Test a bunch of invalid bookmark lines
    #[test]
    fn test_invalid() {
        let reader = Cursor::new(&b"111\n"[..]);
        let bookmarks = StockBookmarks::from_reader(reader);
        assert_matches!(
            bookmarks.unwrap_err().downcast::<ErrorKind>().unwrap(),
            ErrorKind::InvalidBookmarkLine(_)
        );

        // no space or bookmark name
        let reader = Cursor::new(&b"1111111111111111111111111111111111111111\n"[..]);
        let bookmarks = StockBookmarks::from_reader(reader);
        assert_matches!(
            bookmarks.unwrap_err().downcast::<ErrorKind>().unwrap(),
            ErrorKind::InvalidBookmarkLine(_)
        );

        // no bookmark name
        let reader = Cursor::new(&b"1111111111111111111111111111111111111111 \n"[..]);
        let bookmarks = StockBookmarks::from_reader(reader);
        assert_matches!(
            bookmarks.unwrap_err().downcast::<ErrorKind>().unwrap(),
            ErrorKind::InvalidBookmarkLine(_)
        );

        // no space after hash
        let reader = Cursor::new(&b"1111111111111111111111111111111111111111ab\n"[..]);
        let bookmarks = StockBookmarks::from_reader(reader);
        assert_matches!(
            bookmarks.unwrap_err().downcast::<ErrorKind>().unwrap(),
            ErrorKind::InvalidBookmarkLine(_)
        );

        // short hash
        let reader = Cursor::new(&b"111111111111111111111111111111111111111  1ab\n"[..]);
        let bookmarks = StockBookmarks::from_reader(reader);
        let err = bookmarks.unwrap_err();
        match err.downcast::<Context<ErrorKind>>() {
            Ok(ctxt) => match ctxt.get_context() {
                ok @ &ErrorKind::InvalidHash(..) => println!("OK: {:?}", ok),
                bad => panic!("unexpected error {}", bad),
            },
            Err(bad) => panic!("other error: {:?}", bad),
        };

        // non-ASCII
        let reader = Cursor::new(&b"111111111111111111111111111111111111111\xff test\n"[..]);
        let err = StockBookmarks::from_reader(reader).unwrap_err();
        match err.downcast::<Context<ErrorKind>>() {
            Ok(ctxt) => match ctxt.get_context() {
                ok @ &ErrorKind::InvalidHash(..) => println!("OK: {:?}", ok),
                bad => panic!("unexpected error {}", bad),
            },
            Err(bad) => panic!("other error: {:?}", bad),
        };

        // not a valid hex string
        let reader = Cursor::new(&b"abcdefgabcdefgabcdefgabcdefgabcdefgabcde test\n"[..]);
        let err = StockBookmarks::from_reader(reader).unwrap_err();
        match err.downcast::<Context<ErrorKind>>() {
            Ok(ctxt) => match ctxt.get_context() {
                ok @ &ErrorKind::InvalidHash(..) => println!("OK: {:?}", ok),
                bad => panic!("unexpected error {}", bad),
            },
            Err(bad) => panic!("other error: {:?}", bad),
        };
    }
}
