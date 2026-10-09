// New file (not from upstream Alacritty or ayosec's graphics patch). See ../../../PROVENANCE.md.

//! Kitty graphics protocol (`ESC _ G … ESC \`).
//!
//! Reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>. Written from the
//! specification; no code was taken from kitty or other implementations.
//!
//! Supported:
//!
//! - Transmission (`a=t`, `a=T`) of PNG (`f=100`) and raw RGB/RGBA (`f=24`/`f=32` with `s`,
//!   `v`) data sent inline (`t=d`), in one string or in chunks (`m=1` … `m=0`), stored by image
//!   id (`i`) or image number (`I`, which gets a terminal-assigned id).
//! - Queries (`a=q`), in one string or in chunks, answered without storing anything.
//! - Virtual placements for Unicode placeholders (`a=p,U=1` and `a=T,U=1`), and direct
//!   placements at the cursor (`a=p`, `a=T`) scaled to `c`×`r` cells and cropped with `x`, `y`,
//!   `w`, `h`. `C=1` keeps the cursor; otherwise it ends on the last row of the image, after its
//!   last column.
//! - Deletion (`a=d`) with every `d` value except animation frames, which do not exist here.
//! - Replies `ESC _ G i=<id>[,I=<number>][,p=<placement>];OK ESC \` or an error, honoring `q`.
//!
//! Not supported (answered with `EINVAL`): file and shared memory transmission (`t=f/t/s`),
//! compression (`o=z`), animation (`a=f/a/c`) and relative placements (`P`, `Q`). Pixel offsets
//! inside the first cell (`X`, `Y`) and the z-index are ignored when drawing; the z-index is only
//! used by deletions. One image store serves both screens.
//!
//! Images are decoded synchronously on the PTY thread, like OSC 1337 images, under the same
//! decoder limits. A placement converts the stored image into its own texture, already scaled
//! to the cell size of the terminal at placement time, which is uploaded through the retained
//! queue of [`Graphics`]. Direct placements attach that texture to the grid cells like Sixel
//! graphics do; deleting one queues the removal of its texture, and the cells that still
//! reference it draw nothing. Every command that adds, replaces or removes a placement marks the
//! whole screen as damaged, since placements are not cell contents.
//!
//! # Memory
//!
//! Kitty graphics hold at most:
//!
//! - the APC string being received, up to `MAX_APC_SIZE` bytes (about 85 MiB);
//! - one incomplete transmission and one incomplete query, `MAX_DATA_SIZE` (64 MiB) each;
//! - stored images: `STORAGE_QUOTA` (320 MiB) of RGBA pixels and `IMAGE_LIMIT` (4096)
//!   images. A new image evicts the oldest images without placements first, then the oldest
//!   images with placements, together with their placements;
//! - placement textures: `PLACEMENT_QUOTA` (256 MiB) of RGBA pixels and `PLACEMENT_LIMIT`
//!   (4096) placements, virtual and direct. A new placement evicts the oldest placements, and
//!   direct placements whose cells are all gone stop counting. Virtual placements of the same
//!   image, source rectangle and layout share one texture, counted once. A texture waits in
//!   the retained queue (CPU memory) until the display uploads it, then lives only on the GPU,
//!   where the display keeps it until it is removed: unlike Sixel and OSC 1337 graphics, it is
//!   neither dropped from the queue nor evicted by the display.
//!
//! Source images and placement textures have separate quotas: they live in different memory
//! (CPU and GPU), and an image is often transmitted long before it is placed, so new
//! placements must not evict stored images, and new images must only remove the placements of
//! the images they evict.
//!
//! The quotas count the pixels of live images and live placements, not resident memory. On
//! top of them, removed textures stay allocated until the display processes their removal,
//! new textures can wait in the retained queue meanwhile (so replacing every placement before
//! the next frame briefly holds both sets), an upload overlaps its CPU buffer with the GPU
//! allocation, and decoding and scaling need temporaries. Direct placements whose cells are all
//! gone keep counting until the next direct placement or an over-quota reservation drops them.

use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Weak};
use std::{cmp, fmt, mem, str};

use base64::Engine;
use image::{imageops, RgbaImage};
use parking_lot::Mutex;

use super::osc1337::{box_shrink, decode_image, enlarge, BASE64, MAX_DECODED_DIMENSION};
use super::{
    attach_graphic_row, ColorType, GraphicData, GraphicId, Graphics, TextureOperation, TextureRef,
    MAX_GRAPHIC_DIMENSIONS,
};
use crate::event::EventListener;
use crate::grid::Dimensions;
use crate::index::{Column, Line};
use crate::term::Term;
use crate::vte::ansi::Handler;

/// Largest image payload after base64 decoding, in bytes: a 4096x4096 RGBA image.
const MAX_DATA_SIZE: usize = 64 * 1024 * 1024;

/// Largest buffered APC string: the base64 form of [`MAX_DATA_SIZE`] bytes plus control data.
const MAX_APC_SIZE: usize = MAX_DATA_SIZE / 3 * 4 + 4096;

/// Default storage quota for decoded images, in bytes (kitty uses 320 MB per screen).
const STORAGE_QUOTA: usize = 320 * 1024 * 1024;

/// Most images stored at once, which bounds the bookkeeping of tiny images.
const IMAGE_LIMIT: usize = 4096;

/// Quota for the placement textures, in bytes of RGBA pixels: four textures of the largest
/// size, [`MAX_GRAPHIC_DIMENSIONS`].
const PLACEMENT_QUOTA: usize = 256 * 1024 * 1024;

/// Most placements at once, virtual and direct, which bounds the number of GPU textures.
const PLACEMENT_LIMIT: usize = 4096;

/// First id assigned by the terminal to images transmitted with an image number (`I`) only.
const FIRST_ASSIGNED_ID: u32 = 1 << 31;

/// An image prepared for Unicode placeholder cells (`a=p,U=1` virtual placement).
///
/// The texture holds the image already fitted into `columns × rows` cells (aspect ratio kept,
/// centred, transparent padding), so the cell at placeholder row `r`, column `c` shows the
/// texture sub-rectangle `(c · tile_width, r · tile_height, tile_width, tile_height)`.
#[derive(Clone, Debug)]
pub struct VirtualPlacement {
    /// Texture uploaded through the retained queue; held here so it lives as long as the
    /// placement, independent of any grid cell. Placements showing the same pixels share it.
    pub texture: Arc<TextureRef>,

    /// Placeholder grid size in cells.
    pub columns: u32,
    pub rows: u32,

    /// Texture pixels covered by one placeholder cell.
    pub tile_width: f32,
    pub tile_height: f32,
}

/// Kitty graphics state of one terminal.
pub struct Kitty {
    /// Virtual placements by `(image id, placement id)`.
    virtual_placements: BTreeMap<(u32, u32), VirtualEntry>,

    /// Placements drawn on the grid by `(image id, placement id, serial)`.
    direct_placements: BTreeMap<(u32, u32, u64), DirectPlacement>,

    /// Every placement by serial, oldest first: the eviction order.
    placement_order: BTreeMap<u64, PlacementKey>,

    /// Textures of virtual placements, shared by the placements with the same key.
    shared_textures: HashMap<TextureKey, SharedTexture>,

    /// Bytes of the placement textures, each shared texture counted once.
    placement_bytes: usize,

    /// Limits for `placement_bytes` and for the number of placements.
    placement_quota: usize,
    placement_limit: usize,

    /// Stored images by image id.
    images: HashMap<u32, Image>,

    /// Ids of the stored images by serial, oldest first.
    image_order: BTreeMap<u64, u32>,

    /// Bytes of pixel data in `images`.
    stored_bytes: usize,

    /// Limits for `stored_bytes` and for the number of images.
    storage_quota: usize,
    image_limit: usize,

    /// Last serial given to an image or a placement.
    last_serial: u64,

    /// Last id assigned to an image transmitted with an image number.
    last_assigned_id: u32,

    /// Chunked transmission waiting for more data.
    transfer: Option<Transfer>,

    /// Chunked query waiting for more data. Continuation chunks go to it before `transfer`.
    query: Option<Transfer>,

    /// APC string being received.
    apc: Apc,

    /// Placements changed since the last [`Kitty::take_damage`].
    damaged: bool,
}

impl Default for Kitty {
    fn default() -> Self {
        Self {
            virtual_placements: BTreeMap::new(),
            direct_placements: BTreeMap::new(),
            placement_order: BTreeMap::new(),
            shared_textures: HashMap::new(),
            placement_bytes: 0,
            placement_quota: PLACEMENT_QUOTA,
            placement_limit: PLACEMENT_LIMIT,
            images: HashMap::new(),
            image_order: BTreeMap::new(),
            stored_bytes: 0,
            storage_quota: STORAGE_QUOTA,
            image_limit: IMAGE_LIMIT,
            last_serial: 0,
            last_assigned_id: 0,
            transfer: None,
            query: None,
            apc: Apc::Idle,
            damaged: false,
        }
    }
}

impl fmt::Debug for Kitty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kitty")
            .field("virtual_placements", &self.virtual_placements.len())
            .field("direct_placements", &self.direct_placements.len())
            .field("placement_bytes", &self.placement_bytes)
            .field("images", &self.images.len())
            .field("stored_bytes", &self.stored_bytes)
            .field("transfer", &self.transfer.is_some())
            .field("query", &self.query.is_some())
            .finish_non_exhaustive()
    }
}

impl Kitty {
    /// Virtual placement drawn by a placeholder cell of `image_id` and `placement_id`.
    ///
    /// Placement id 0 in a placeholder matches any virtual placement of the image; the lowest
    /// placement id wins.
    pub fn virtual_placement(&self, image_id: u32, placement_id: u32) -> Option<&VirtualPlacement> {
        let entry = if placement_id != 0 {
            self.virtual_placements.get(&(image_id, placement_id))
        } else {
            let ids = (image_id, 0)..=(image_id, u32::MAX);
            self.virtual_placements.range(ids).next().map(|(_, entry)| entry)
        };

        entry.map(|entry| &entry.placement)
    }

    /// Whether placements were added, replaced or removed since the last call.
    ///
    /// Placements are not cell contents, so the terminal damages the whole screen for them.
    pub(crate) fn take_damage(&mut self) -> bool {
        mem::take(&mut self.damaged)
    }

    /// Start of an APC string.
    pub(crate) fn apc_start(&mut self) {
        self.apc = Apc::Start;
    }

    /// Bytes of the current APC string. Only strings starting with `G` are kept.
    pub(crate) fn apc_put(&mut self, bytes: &[u8]) {
        match &mut self.apc {
            Apc::Start => match bytes.split_first() {
                Some((b'G', rest)) => {
                    self.apc = Apc::Graphics(ApcString { data: Vec::new(), overflow: false });
                    self.apc_put(rest);
                },
                Some(_) => self.apc = Apc::Ignored,
                None => (),
            },
            Apc::Graphics(string) => {
                let room = MAX_APC_SIZE - string.data.len();
                if bytes.len() > room {
                    string.overflow = true;
                }
                string.data.extend_from_slice(&bytes[..cmp::min(room, bytes.len())]);
            },
            Apc::Idle | Apc::Ignored => (),
        }
    }

    /// End of the current APC string; returns it if it is a graphics command.
    pub(crate) fn apc_end(&mut self) -> Option<ApcString> {
        match mem::replace(&mut self.apc, Apc::Idle) {
            Apc::Graphics(string) => Some(string),
            Apc::Idle | Apc::Start | Apc::Ignored => None,
        }
    }

    /// Whether the image has a virtual placement or a direct placement still on the grid.
    fn has_placements(&self, image_id: u32) -> bool {
        self.virtual_placements.range((image_id, 0)..=(image_id, u32::MAX)).next().is_some()
            || self
                .direct_placements
                .range((image_id, 0, 0)..=(image_id, u32::MAX, u64::MAX))
                .any(|(_, placement)| placement.is_alive())
    }

    fn next_serial(&mut self) -> u64 {
        self.last_serial += 1;
        self.last_serial
    }

    /// Newest stored image transmitted with `number`.
    fn newest_with_number(&self, number: u32) -> Option<u32> {
        self.images
            .iter()
            .filter(|(_, image)| image.number == number)
            .max_by_key(|(_, image)| image.serial)
            .map(|(&id, _)| id)
    }

    /// Pick an unused id for an image transmitted with an image number only.
    fn assign_id(&mut self) -> u32 {
        loop {
            self.last_assigned_id = match self.last_assigned_id {
                id if id < FIRST_ASSIGNED_ID || id == u32::MAX => FIRST_ASSIGNED_ID,
                id => id + 1,
            };

            let id = self.last_assigned_id;
            if !self.images.contains_key(&id) && !self.has_placements(id) {
                return id;
            }
        }
    }

    /// Store an image, replacing the image with the same id and its placements, and evicting
    /// old images if the quota or the image limit is exceeded.
    fn store(&mut self, operations: &Mutex<Vec<TextureOperation>>, id: u32, image: Image) {
        self.remove_image(operations, id);
        self.evict_images(operations, image.size());

        let serial = self.next_serial();
        self.stored_bytes += image.size();
        self.image_order.insert(serial, id);
        self.images.insert(id, Image { serial, ..image });
    }

    /// Evict images until one more image of `size` bytes fits: the oldest images without
    /// placements first, then the oldest images with their placements.
    ///
    /// The candidates are picked in one ordered pass, and every eviction costs O(log n) per
    /// removed placement, so evicting many images costs O(n log n), not a scan per image.
    fn evict_images(&mut self, operations: &Mutex<Vec<TextureOperation>>, size: usize) {
        let fits = |kitty: &Self| {
            kitty.stored_bytes + size <= kitty.storage_quota
                && kitty.images.len() < kitty.image_limit
        };
        if fits(self) {
            return;
        }

        let (unplaced, placed): (Vec<u32>, Vec<u32>) =
            self.image_order.values().copied().partition(|&id| !self.has_placements(id));
        for id in unplaced.into_iter().chain(placed) {
            if fits(self) {
                break;
            }

            log::debug!("Evicting kitty image {id} to stay under the storage quota");
            self.remove_image(operations, id);
        }
    }

    /// Remove an image and all its placements.
    fn remove_image(&mut self, operations: &Mutex<Vec<TextureOperation>>, id: u32) {
        self.delete_image_placements(operations, id, 0);
        self.free_image(id);
    }

    /// Release the pixel data of an image.
    fn free_image(&mut self, id: u32) {
        if let Some(image) = self.images.remove(&id) {
            self.stored_bytes -= image.size();
            self.image_order.remove(&image.serial);
        }
    }

    /// Release the pixel data of every image without placements.
    fn free_unplaced_images(&mut self) {
        let unplaced: Vec<u32> =
            self.images.keys().copied().filter(|&id| !self.has_placements(id)).collect();
        for id in unplaced {
            self.free_image(id);
        }
    }

    /// Delete the placements of an image (only `placement_id` unless it is 0), virtual ones
    /// included.
    fn delete_image_placements(
        &mut self,
        operations: &Mutex<Vec<TextureOperation>>,
        image_id: u32,
        placement_id: u32,
    ) {
        let (first, last) = match placement_id {
            0 => (0, u32::MAX),
            id => (id, id),
        };

        while let Some((&key, _)) =
            self.virtual_placements.range((image_id, first)..=(image_id, last)).next()
        {
            self.remove_virtual(key);
        }

        while let Some((&key, _)) =
            self.direct_placements.range((image_id, first, 0)..=(image_id, last, u64::MAX)).next()
        {
            self.remove_direct(operations, key);
        }
    }

    /// Delete the direct placements matching `filter`. Returns the ids of their images.
    fn delete_direct_placements(
        &mut self,
        operations: &Mutex<Vec<TextureOperation>>,
        filter: impl Fn(&DirectPlacement) -> bool,
    ) -> HashSet<u32> {
        let keys: Vec<(u32, u32, u64)> = self
            .direct_placements
            .iter()
            .filter(|(_, placement)| filter(placement))
            .map(|(&key, _)| key)
            .collect();

        let images = keys.iter().map(|&(image, ..)| image).collect();
        for key in keys {
            self.remove_direct(operations, key);
        }

        images
    }

    /// Forget the direct placements whose cells are all gone; their textures went with them.
    fn forget_dead_direct_placements(&mut self) {
        let Self { direct_placements, placement_order, placement_bytes, .. } = self;
        direct_placements.retain(|&(_, _, serial), placement| {
            if placement.is_alive() {
                return true;
            }

            placement_order.remove(&serial);
            *placement_bytes -= placement.bytes;
            false
        });
    }

    /// Evict the oldest placements until one more placement, with a new texture of `bytes`
    /// (0 for a shared texture), fits in the placement quota and limit.
    fn reserve_placement(&mut self, operations: &Mutex<Vec<TextureOperation>>, bytes: usize) {
        let fits = |kitty: &Self| {
            kitty.placement_bytes + bytes <= kitty.placement_quota
                && kitty.placement_order.len() < kitty.placement_limit
        };
        if fits(self) {
            return;
        }

        self.forget_dead_direct_placements();
        while !fits(self) {
            let Some((_, &key)) = self.placement_order.first_key_value() else { break };
            log::debug!("Evicting kitty placement {key:?} to stay under the placement quota");
            match key {
                PlacementKey::Virtual(key) => self.remove_virtual(key),
                PlacementKey::Direct(key) => self.remove_direct(operations, key),
            }
        }
    }

    /// The texture of a virtual placement with `key`, if any, counting one more user.
    fn acquire_texture(&mut self, key: &TextureKey) -> Option<Arc<TextureRef>> {
        let shared = self.shared_textures.get_mut(key)?;
        let texture = shared.texture.upgrade()?;
        shared.users += 1;
        Some(texture)
    }

    /// Record a virtual placement. Its texture is already counted in `shared_textures`, and
    /// no placement has its ids.
    fn add_virtual(&mut self, ids: (u32, u32), placement: VirtualPlacement, key: TextureKey) {
        let serial = self.next_serial();
        self.placement_order.insert(serial, PlacementKey::Virtual(ids));
        let replaced =
            self.virtual_placements.insert(ids, VirtualEntry { placement, serial, texture: key });
        debug_assert!(replaced.is_none(), "virtual placement {ids:?} was not removed");
        self.damaged = true;
    }

    /// Record a direct placement of `image_id` and `placement_id`.
    fn add_direct(&mut self, image_id: u32, placement_id: u32, placement: DirectPlacement) {
        let serial = self.next_serial();
        self.placement_bytes += placement.bytes;
        self.placement_order.insert(serial, PlacementKey::Direct((image_id, placement_id, serial)));
        self.direct_placements.insert((image_id, placement_id, serial), placement);
        self.damaged = true;
    }

    /// Remove a virtual placement. Dropping the last reference to its texture queues the
    /// removal of the texture (`TextureRef::drop`).
    fn remove_virtual(&mut self, ids: (u32, u32)) {
        let Some(entry) = self.virtual_placements.remove(&ids) else { return };
        self.placement_order.remove(&entry.serial);
        if let Entry::Occupied(mut shared) = self.shared_textures.entry(entry.texture) {
            shared.get_mut().users -= 1;
            if shared.get().users == 0 {
                self.placement_bytes -= shared.remove().bytes;
            }
        }

        self.damaged = true;
    }

    /// Remove a direct placement and its texture.
    fn remove_direct(&mut self, operations: &Mutex<Vec<TextureOperation>>, key: (u32, u32, u64)) {
        let Some(placement) = self.direct_placements.remove(&key) else { return };
        self.placement_order.remove(&key.2);
        self.placement_bytes -= placement.bytes;

        // The cells referencing the texture stay on the grid and draw nothing. When they are
        // dropped, the texture is removed again, which the display ignores.
        if placement.is_alive() {
            operations.lock().push(TextureOperation::Remove(placement.graphic_id));
            self.damaged = true;
        }
    }
}

/// A transmitted image.
struct Image {
    /// Image number (`I`) it was transmitted with, or 0.
    number: u32,

    /// Decoded pixels.
    pixels: Arc<RgbaImage>,

    /// Transmission order.
    serial: u64,
}

impl Image {
    /// Bytes of pixel data.
    fn size(&self) -> usize {
        self.pixels.as_raw().len()
    }
}

/// A placement drawn on the grid cells.
#[derive(Debug)]
struct DirectPlacement {
    z_index: i32,

    /// Texture attached to the cells.
    graphic_id: GraphicId,
    texture: Weak<TextureRef>,

    /// Bytes of the texture, counted in the placement quota until the placement is removed or
    /// found dead.
    bytes: usize,
}

impl DirectPlacement {
    /// Whether any cell still references the texture.
    fn is_alive(&self) -> bool {
        self.texture.strong_count() > 0
    }
}

/// A virtual placement and its bookkeeping.
struct VirtualEntry {
    placement: VirtualPlacement,

    /// Key in [`Kitty::placement_order`].
    serial: u64,

    /// Key in [`Kitty::shared_textures`].
    texture: TextureKey,
}

/// A placement, as found in [`Kitty::placement_order`].
#[derive(Clone, Copy, Debug)]
enum PlacementKey {
    /// Key in [`Kitty::virtual_placements`].
    Virtual((u32, u32)),

    /// Key in [`Kitty::direct_placements`].
    Direct((u32, u32, u64)),
}

/// What the texture of a virtual placement shows: placements with equal keys share it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TextureKey {
    image_id: u32,
    source: Rect,
    layout: Layout,

    /// Cell size at placement time, in pixels.
    cell: (u32, u32),
}

/// A texture shared by virtual placements.
///
/// Invariant: while `users` > 0, that many [`VirtualEntry`] values hold the texture, so
/// `texture` can be upgraded; its `bytes` are counted once in [`Kitty::placement_bytes`].
struct SharedTexture {
    texture: Weak<TextureRef>,
    users: usize,
    bytes: usize,
}

/// State of the APC string being received.
#[derive(Debug)]
enum Apc {
    /// No APC string.
    Idle,

    /// Started; its first byte selects the command type.
    Start,

    /// A graphics command (`G`).
    Graphics(ApcString),

    /// Another APC string, discarded.
    Ignored,
}

/// A graphics command, without the leading `G`.
#[derive(Debug)]
pub(crate) struct ApcString {
    data: Vec<u8>,

    /// The string exceeded [`MAX_APC_SIZE`] and was truncated.
    overflow: bool,
}

/// Chunked transmission in progress.
struct Transfer {
    /// Control data of the first chunk.
    command: Command,

    /// Decoded payload.
    data: Vec<u8>,

    /// Base64 characters of an incomplete group at the end of the last chunk.
    carry: Vec<u8>,

    /// The transmission failed; later chunks are discarded.
    error: Option<Error>,
}

impl Transfer {
    fn new(command: Command) -> Self {
        Self { command, data: Vec::new(), carry: Vec::new(), error: None }
    }

    /// Add a chunk of base64 data.
    fn push(&mut self, payload: &[u8], overflow: bool) {
        if self.error.is_some() {
            return;
        }

        let result = if overflow { Err(Error::TooLarge) } else { self.decode(payload) };
        if let Err(err) = result {
            self.error = Some(err);
            self.data = Vec::new();
            self.carry.clear();
        }
    }

    /// Decode whole base64 groups, keeping an incomplete group for the next chunk.
    fn decode(&mut self, mut payload: &[u8]) -> Result<(), Error> {
        if !self.carry.is_empty() {
            let take = cmp::min(4 - self.carry.len(), payload.len());
            self.carry.extend_from_slice(&payload[..take]);
            payload = &payload[take..];
            if self.carry.len() < 4 {
                return Ok(());
            }

            let group = mem::take(&mut self.carry);
            self.decode_groups(&group)?;
        }

        let whole = payload.len() / 4 * 4;
        self.decode_groups(&payload[..whole])?;
        self.carry.extend_from_slice(&payload[whole..]);
        Ok(())
    }

    fn decode_groups(&mut self, base64: &[u8]) -> Result<(), Error> {
        if self.data.len() + base64.len() / 4 * 3 > MAX_DATA_SIZE {
            return Err(Error::TooLarge);
        }

        BASE64.decode_vec(base64, &mut self.data).map_err(|_| Error::Invalid("invalid base64 data"))
    }

    /// Complete the transmission.
    fn finish(mut self) -> (Command, Result<Vec<u8>, Error>) {
        if self.error.is_none() && !self.carry.is_empty() {
            let carry = mem::take(&mut self.carry);
            if let Err(err) = self.decode_groups(&carry) {
                self.error = Some(err);
            }
        }

        let result = match self.error {
            Some(err) => Err(err),
            None => Ok(self.data),
        };
        (self.command, result)
    }
}

/// Reason for rejecting a command, sent in the reply.
#[derive(Debug, PartialEq, Eq)]
enum Error {
    /// The image does not exist (`ENOENT`).
    NotFound,

    /// Invalid or unsupported command (`EINVAL`).
    Invalid(&'static str),

    /// Raw pixel data is shorter than the image size (`ENODATA`).
    NoData,

    /// The data is not a valid image, or exceeds the decoder limits (`EBADPNG`).
    BadImage(String),

    /// The data exceeds [`MAX_DATA_SIZE`] (`EFBIG`).
    TooLarge,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound => f.write_str("ENOENT:image not found"),
            Error::Invalid(reason) => write!(f, "EINVAL:{reason}"),
            Error::NoData => f.write_str("ENODATA:insufficient image data"),
            Error::BadImage(reason) => {
                // Replies may only contain printable ASCII characters.
                f.write_str("EBADPNG:")?;
                reason
                    .chars()
                    .map(|c| if c == ' ' || c.is_ascii_graphic() { c } else { '?' })
                    .try_for_each(|c| fmt::Write::write_char(f, c))
            },
            Error::TooLarge => f.write_str("EFBIG:image data is too large"),
        }
    }
}

/// Control data of a graphics command.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Command {
    /// `a`: action.
    action: u8,
    /// `q`: response suppression, 0 to 2.
    quiet: u8,
    /// `f`: data format.
    format: u32,
    /// `t`: transmission medium.
    medium: u8,
    /// `o`: compression.
    compression: Option<u8>,
    /// `s`, `v`: size of raw pixel data.
    width: u32,
    height: u32,
    /// `i`: image id.
    image_id: u32,
    /// `I`: image number.
    image_number: u32,
    /// `p`: placement id.
    placement_id: u32,
    /// `m`: more chunks follow.
    more: bool,
    /// `x`, `y`: source rectangle origin in pixels, or cell for deletions (1-based).
    x: u32,
    y: u32,
    /// `w`, `h`: source rectangle size in pixels (0 = up to the image edge).
    source_width: u32,
    source_height: u32,
    /// `c`, `r`: placement size in cells.
    columns: u32,
    rows: u32,
    /// `C=1`: keep the cursor.
    keep_cursor: bool,
    /// `U=1`: virtual placement for Unicode placeholders.
    unicode: bool,
    /// `z`: z-index.
    z_index: i32,
    /// `P`, `Q`: relative placement.
    relative: bool,
    /// `d`: what to delete.
    delete: u8,
    /// Keys present in the control data.
    keys: Keys,
}

impl Default for Command {
    fn default() -> Self {
        Self {
            action: b't',
            quiet: 0,
            format: 32,
            medium: b'd',
            compression: None,
            width: 0,
            height: 0,
            image_id: 0,
            image_number: 0,
            placement_id: 0,
            more: false,
            x: 0,
            y: 0,
            source_width: 0,
            source_height: 0,
            columns: 0,
            rows: 0,
            keep_cursor: false,
            unicode: false,
            z_index: 0,
            relative: false,
            delete: b'a',
            keys: Keys::default(),
        }
    }
}

/// Set of control data keys (ASCII letters).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Keys(u64);

impl Keys {
    fn bit(key: u8) -> u64 {
        match key {
            b'a'..=b'z' => 1 << (key - b'a'),
            b'A'..=b'Z' => 1 << (key - b'A' + 26),
            _ => 0,
        }
    }

    fn insert(&mut self, key: u8) {
        self.0 |= Self::bit(key);
    }

    fn contains(self, key: u8) -> bool {
        self.0 & Self::bit(key) != 0
    }

    /// Whether every key is in `allowed`.
    fn only(self, allowed: &[u8]) -> bool {
        let allowed = allowed.iter().fold(0, |bits, &key| bits | Self::bit(key));
        self.0 & !allowed == 0
    }
}

impl Command {
    /// Parse the control data (`key=value` pairs separated by commas).
    ///
    /// Unknown keys are ignored; malformed pairs and invalid values reject the command.
    fn parse(control: &[u8]) -> Result<Self, ControlError> {
        let mut command = Command::default();

        for pair in control.split(|&byte| byte == b',').filter(|pair| !pair.is_empty()) {
            let invalid = || ControlError(String::from_utf8_lossy(pair).into_owned());
            let (&key, value) = match pair {
                [key, b'=', value @ ..] => (key, value),
                _ => return Err(invalid()),
            };

            let number = || -> Result<u32, ControlError> {
                str::from_utf8(value).ok().and_then(|value| value.parse().ok()).ok_or_else(invalid)
            };
            let signed = || -> Result<i32, ControlError> {
                str::from_utf8(value).ok().and_then(|value| value.parse().ok()).ok_or_else(invalid)
            };
            let character = || match value {
                [value] => Ok(*value),
                _ => Err(invalid()),
            };

            command.keys.insert(key);
            match key {
                b'a' => command.action = character()?,
                b'q' => command.quiet = cmp::min(number()?, 2) as u8,
                b'f' => command.format = number()?,
                b't' => command.medium = character()?,
                b'o' => command.compression = Some(character()?),
                b's' => command.width = number()?,
                b'v' => command.height = number()?,
                b'i' => command.image_id = number()?,
                b'I' => command.image_number = number()?,
                b'p' => command.placement_id = number()?,
                b'm' => command.more = number()? != 0,
                b'x' => command.x = number()?,
                b'y' => command.y = number()?,
                b'w' => command.source_width = number()?,
                b'h' => command.source_height = number()?,
                b'c' => command.columns = number()?,
                b'r' => command.rows = number()?,
                b'C' => command.keep_cursor = number()? == 1,
                b'U' => command.unicode = number()? == 1,
                b'z' => command.z_index = signed()?,
                b'P' | b'Q' => command.relative |= number()? != 0,
                b'd' => command.delete = character()?,
                b'H' | b'V' => {
                    signed()?;
                },
                b'S' | b'O' | b'N' | b'X' | b'Y' => {
                    number()?;
                },
                _ => log::debug!("Ignoring unknown kitty graphics key {:?}", key as char),
            }
        }

        Ok(command)
    }

    /// Whether this command continues the chunked transmission started by `first`.
    ///
    /// Continuation chunks carry only `m` and `q`; the action of the first chunk is accepted
    /// too.
    fn continues(&self, first: &Command) -> bool {
        if self.action == first.action {
            self.keys.only(b"amq")
        } else {
            self.keys.only(b"mq")
        }
    }
}

/// Malformed control data.
#[derive(Debug)]
struct ControlError(String);

/// Handle a graphics command. Returns the reply to write to the PTY, if any.
pub(crate) fn handle<L: EventListener>(term: &mut Term<L>, apc: ApcString) -> Option<String> {
    let (control, payload) = match apc.data.iter().position(|&byte| byte == b';') {
        Some(separator) => (&apc.data[..separator], &apc.data[separator + 1..]),
        None => (&apc.data[..], &[][..]),
    };

    let command = match Command::parse(control) {
        Ok(command) => command,
        Err(ControlError(pair)) => {
            log::debug!("Ignoring kitty graphics command with invalid control data {pair:?}");
            return None;
        },
    };

    // Continuation chunks complete the query being received, if any, or else the transmission.
    let kitty = &mut term.graphics.kitty;
    let active = if kitty.query.is_some() { &mut kitty.query } else { &mut kitty.transfer };
    if let Some(transfer) = active {
        if command.continues(&transfer.command) {
            transfer.push(payload, apc.overflow);
            if command.keys.contains(b'q') {
                transfer.command.quiet = command.quiet;
            }

            if command.more {
                return None;
            }

            let transfer = active.take()?;
            return complete(term, transfer);
        }
    }

    // Checked before any action, so an invalid command can't delete or display anything.
    if command.image_id != 0 && command.image_number != 0 {
        return reply(&command, command.image_id, Err(Error::Invalid("both i and I given")));
    }

    match command.action {
        b'q' | b't' | b'T' => {
            if command.action != b'q' && kitty.transfer.take().is_some() {
                log::debug!("Discarding an incomplete kitty graphics transmission");
            }
            // A query in progress is abandoned by any new query or transmission.
            if kitty.query.take().is_some() {
                log::debug!("Discarding an incomplete kitty graphics query");
            }

            let mut transfer = Transfer::new(command);
            transfer.push(payload, apc.overflow);
            if !transfer.command.more {
                return complete(term, transfer);
            }

            match transfer.command.action {
                b'q' => kitty.query = Some(transfer),
                _ => kitty.transfer = Some(transfer),
            }
            None
        },
        b'p' => {
            let mut image_id = command.image_id;
            let result = put(term, &command, &mut image_id);
            reply(&command, image_id, result)
        },
        b'd' => {
            // Take both slots: a query can be open inside a transmission.
            let transfer = kitty.transfer.take();
            let query = kitty.query.take();
            if transfer.is_some() || query.is_some() {
                log::debug!("Kitty graphics deletion aborts an incomplete transmission");
            }

            delete(term, &command);
            None
        },
        b'f' | b'a' | b'c' => {
            reply(&command, command.image_id, Err(Error::Invalid("animation is not supported")))
        },
        _ => reply(&command, command.image_id, Err(Error::Invalid("unknown action"))),
    }
}

/// Reply to a command, unless suppressed by `q` or the command has no image id or number.
fn reply(command: &Command, image_id: u32, result: Result<(), Error>) -> Option<String> {
    match (&result, command.quiet) {
        (_, 2) | (Ok(()), 1) => return None,
        _ => (),
    }

    if image_id == 0 && command.image_number == 0 {
        return None;
    }

    let mut keys = Vec::with_capacity(3);
    if image_id != 0 {
        keys.push(format!("i={image_id}"));
    }
    if command.image_number != 0 {
        keys.push(format!("I={}", command.image_number));
    }
    if command.placement_id != 0 {
        keys.push(format!("p={}", command.placement_id));
    }

    let message = match result {
        Ok(()) => String::from("OK"),
        Err(err) => err.to_string(),
    };

    Some(format!("\x1b_G{};{message}\x1b\\", keys.join(",")))
}

/// Complete a transmission (`a=t`, `a=T`) or a query (`a=q`, loaded without storing it) and
/// reply.
fn complete<L: EventListener>(term: &mut Term<L>, transfer: Transfer) -> Option<String> {
    let (command, data) = transfer.finish();
    let mut image_id = command.image_id;
    let result = match command.action {
        b'q' => data.and_then(|data| decode_pixels(&command, &data)).map(drop),
        _ => data.and_then(|data| load(term, &command, &data, &mut image_id)),
    };
    reply(&command, image_id, result)
}

/// Decode and store a transmitted image, and display it for `a=T`.
///
/// `image_id` is set to the id assigned to images transmitted with a number.
fn load<L: EventListener>(
    term: &mut Term<L>,
    command: &Command,
    data: &[u8],
    image_id: &mut u32,
) -> Result<(), Error> {
    let pixels = Arc::new(decode_pixels(command, data)?);

    let Graphics { kitty, texture_operations, .. } = &mut term.graphics;
    if command.image_number != 0 {
        *image_id = kitty.assign_id();
    }

    // Images without an id can't be referenced later; `a=T` displays them anyway.
    if *image_id != 0 {
        let image = Image { number: command.image_number, pixels: pixels.clone(), serial: 0 };
        kitty.store(texture_operations, *image_id, image);
    }

    if command.action == b'T' {
        place(term, *image_id, &pixels, command)?;
    }

    Ok(())
}

/// Decode the payload of a transmission.
fn decode_pixels(command: &Command, data: &[u8]) -> Result<RgbaImage, Error> {
    if command.medium != b'd' {
        return Err(Error::Invalid("only direct transmission (t=d) is supported"));
    }

    if command.compression.is_some() {
        return Err(Error::Invalid("compression is not supported"));
    }

    let channels = match command.format {
        100 => {
            let image = decode_image(data).map_err(|err| Error::BadImage(err.to_string()))?;
            return Ok(image.into_rgba8());
        },
        24 => 3,
        32 => 4,
        _ => return Err(Error::Invalid("unknown format")),
    };

    let (width, height) = (command.width, command.height);
    if width == 0 || height == 0 {
        return Err(Error::Invalid("missing image size"));
    }
    if width > MAX_DECODED_DIMENSION || height > MAX_DECODED_DIMENSION {
        return Err(Error::TooLarge);
    }

    let size = width as usize * height as usize * channels;
    let pixels = data.get(..size).ok_or(Error::NoData)?;
    let pixels = if channels == 4 {
        pixels.to_vec()
    } else {
        pixels.chunks_exact(3).flat_map(|rgb| [rgb[0], rgb[1], rgb[2], u8::MAX]).collect()
    };

    Ok(RgbaImage::from_raw(width, height, pixels).expect("buffer matches the image size"))
}

/// Display a stored image (`a=p`).
fn put<L: EventListener>(
    term: &mut Term<L>,
    command: &Command,
    image_id: &mut u32,
) -> Result<(), Error> {
    let kitty = &term.graphics.kitty;
    *image_id = match (command.image_id, command.image_number) {
        (0, 0) => return Err(Error::Invalid("no image id")),
        (0, number) => kitty.newest_with_number(number).ok_or(Error::NotFound)?,
        // `handle` rejects commands with both.
        (id, _) => id,
    };

    let pixels = kitty.images.get(image_id).ok_or(Error::NotFound)?.pixels.clone();
    place(term, *image_id, &pixels, command)
}

/// Create a placement of `image`.
fn place<L: EventListener>(
    term: &mut Term<L>,
    image_id: u32,
    image: &RgbaImage,
    command: &Command,
) -> Result<(), Error> {
    if command.relative {
        return Err(Error::Invalid("relative placements are not supported"));
    }

    let cell = cell_size(&term.graphics)?;
    let source = source_rect(image, command)?;
    let layout = Layout::new(
        (source.width, source.height),
        (command.columns, command.rows),
        cell,
        command.unicode,
    );

    // Placement ids only exist for images with an id.
    let placement_id = if image_id == 0 { 0 } else { command.placement_id };

    if !command.unicode {
        place_direct(term, (image_id, placement_id), image, source, layout, command);
    } else if image_id != 0 {
        // A placeholder can't refer to an image without id.
        let key = TextureKey { image_id, source, layout, cell };
        place_virtual(term, placement_id, image, key);
    }

    Ok(())
}

/// Create or replace a virtual placement, sharing the texture of identical placements.
fn place_virtual<L: EventListener>(
    term: &mut Term<L>,
    placement_id: u32,
    image: &RgbaImage,
    key: TextureKey,
) {
    let ids = (key.image_id, placement_id);
    let Graphics { kitty, texture_operations, .. } = &mut term.graphics;

    // Taken before the replaced placement goes, so an identical replacement keeps its texture.
    let shared = kitty.acquire_texture(&key);

    // A placement replaces the placement with the same ids, a direct one too unless the
    // placement id is 0.
    match placement_id {
        0 => kitty.remove_virtual(ids),
        _ => kitty.delete_image_placements(texture_operations, ids.0, ids.1),
    }

    let texture = match shared {
        Some(texture) => {
            kitty.reserve_placement(texture_operations, 0);
            texture
        },
        None => {
            let bytes = key.layout.bytes();
            kitty.reserve_placement(texture_operations, bytes);

            let (texture, graphic) =
                placement_texture(&mut term.graphics, image, key.source, &key.layout);
            term.graphics.push_retained(graphic);

            let kitty = &mut term.graphics.kitty;
            let shared = SharedTexture { texture: Arc::downgrade(&texture), users: 1, bytes };
            kitty.shared_textures.insert(key, shared);
            kitty.placement_bytes += bytes;
            texture
        },
    };

    let layout = key.layout;
    let placement = VirtualPlacement {
        texture,
        columns: layout.columns,
        rows: layout.rows,
        tile_width: layout.width as f32 / layout.columns as f32,
        tile_height: layout.height as f32 / layout.rows as f32,
    };
    term.graphics.kitty.add_virtual(ids, placement, key);
}

/// Create a direct placement at the cursor, replacing the placements with the same ids.
fn place_direct<L: EventListener>(
    term: &mut Term<L>,
    (image_id, placement_id): (u32, u32),
    image: &RgbaImage,
    source: Rect,
    layout: Layout,
    command: &Command,
) {
    let Graphics { kitty, texture_operations, .. } = &mut term.graphics;
    if placement_id != 0 {
        kitty.delete_image_placements(texture_operations, image_id, placement_id);
    }

    kitty.forget_dead_direct_placements();
    let bytes = layout.bytes();
    kitty.reserve_placement(texture_operations, bytes);

    let (texture, graphic) = placement_texture(&mut term.graphics, image, source, &layout);
    insert_direct(term, &texture, graphic, layout.columns, command.keep_cursor);

    let placement = DirectPlacement {
        z_index: command.z_index,
        graphic_id: texture.id,
        texture: Arc::downgrade(&texture),
        bytes,
    };
    term.graphics.kitty.add_direct(image_id, placement_id, placement);
}

/// Render the texture of a placement. Returns the reference that keeps it in the display and
/// the pixels to upload.
fn placement_texture(
    graphics: &mut Graphics,
    image: &RgbaImage,
    source: Rect,
    layout: &Layout,
) -> (Arc<TextureRef>, GraphicData) {
    let (pixels, is_opaque) = render(image, source, layout);
    let id = graphics.next_id();
    let graphic = GraphicData {
        id,
        width: layout.width as usize,
        height: layout.height as usize,
        color_type: ColorType::Rgba,
        pixels,
        is_opaque,
    };

    let texture = Arc::new(TextureRef {
        id,
        width: layout.width as u16,
        height: layout.height as u16,
        cell_height: graphics.cell_height as usize,
        texture_operations: Arc::downgrade(&graphics.texture_operations),
    });

    (texture, graphic)
}

/// Attach a direct placement to the grid at the cursor.
///
/// With `keep_cursor`, rows below the screen are clipped and the cursor does not move.
/// Otherwise the screen scrolls as needed, and the cursor ends on the last row of the image,
/// `columns` to the right of its start (clamped to the last column).
fn insert_direct<L: EventListener>(
    term: &mut Term<L>,
    texture: &Arc<TextureRef>,
    graphic: GraphicData,
    columns: u32,
    keep_cursor: bool,
) {
    let cell_height = term.graphics.cell_height as usize;
    let start = term.grid().cursor.point;
    let screen_lines = term.screen_lines() as i32;
    let no_skip = HashSet::new();

    for (row, offset_y) in (0usize..).zip((0..texture.height).step_by(cell_height)) {
        let line = if keep_cursor {
            let line = start.line + row;
            if line.0 >= screen_lines {
                break;
            }
            line
        } else {
            if row > 0 {
                term.linefeed();
            }
            term.grid().cursor.point.line
        };

        attach_graphic_row(term, line, start.column.0, texture, &graphic, offset_y, &no_skip);
    }

    if !keep_cursor {
        term.move_forward(columns as usize);
    }

    // Retained: the placement quota bounds it, and the display must not drop it.
    term.graphics.push_retained(graphic);
}

/// Cell size in whole pixels, as used to attach graphics to cells.
fn cell_size(graphics: &Graphics) -> Result<(u32, u32), Error> {
    match (graphics.cell_width as u32, graphics.cell_height as u32) {
        (0, _) | (_, 0) => Err(Error::Invalid("the terminal has no cell size")),
        size => Ok(size),
    }
}

/// Rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Source rectangle (`x`, `y`, `w`, `h`) intersected with the image.
fn source_rect(image: &RgbaImage, command: &Command) -> Result<Rect, Error> {
    let (image_width, image_height) = image.dimensions();
    let x = cmp::min(command.x, image_width);
    let y = cmp::min(command.y, image_height);
    let clip = |size: u32, available: u32| match size {
        0 => available,
        size => cmp::min(size, available),
    };

    let width = clip(command.source_width, image_width - x);
    let height = clip(command.source_height, image_height - y);
    if width == 0 || height == 0 {
        return Err(Error::Invalid("empty source rectangle"));
    }

    Ok(Rect { x, y, width, height })
}

/// Size of a placement and of its texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Layout {
    /// Placement size in cells.
    columns: u32,
    rows: u32,

    /// Texture size in pixels.
    width: u32,
    height: u32,

    /// Scaled source image inside the texture.
    content: Rect,
}

impl Layout {
    /// Lay out a source of `source` pixels in `columns × rows` cells (0 = computed).
    ///
    /// With only one of them, the other follows the aspect ratio; with none, the image keeps
    /// its size. Virtual placements, and placements with both, fit the image into the cells,
    /// centred with transparent padding. The texture is scaled down uniformly to stay within
    /// [`MAX_GRAPHIC_DIMENSIONS`].
    fn new(source: (u32, u32), cells: (u32, u32), cell: (u32, u32), fill_cells: bool) -> Self {
        let (source_width, source_height) = (f64::from(source.0), f64::from(source.1));
        let (cell_width, cell_height) = (f64::from(cell.0), f64::from(cell.1));
        let count = |pixels: f64, cell: f64| (pixels / cell).ceil().clamp(1., f64::from(u32::MAX));

        let (columns, rows, mut content) = match cells {
            (0, 0) => (
                count(source_width, cell_width),
                count(source_height, cell_height),
                (source_width, source_height),
            ),
            (columns, 0) => {
                let width = f64::from(columns) * cell_width;
                let height = source_height * width / source_width;
                (f64::from(columns), count(height, cell_height), (width, height))
            },
            (0, rows) => {
                let height = f64::from(rows) * cell_height;
                let width = source_width * height / source_height;
                (count(width, cell_width), f64::from(rows), (width, height))
            },
            (columns, rows) => (f64::from(columns), f64::from(rows), (0., 0.)),
        };

        let fill_cells = fill_cells || (cells.0 != 0 && cells.1 != 0);
        let (box_width, box_height) = if fill_cells {
            let size = (columns * cell_width, rows * cell_height);
            let scale = f64::min(size.0 / source_width, size.1 / source_height);
            content = (source_width * scale, source_height * scale);
            size
        } else {
            content
        };

        let max = (MAX_GRAPHIC_DIMENSIONS[0] as f64, MAX_GRAPHIC_DIMENSIONS[1] as f64);
        let scale = f64::min(1., f64::min(max.0 / box_width, max.1 / box_height));

        // Every value is clamped to `1..=MAX_GRAPHIC_DIMENSIONS`, so the casts are lossless.
        let width = (box_width * scale).round().clamp(1., max.0) as u32;
        let height = (box_height * scale).round().clamp(1., max.1) as u32;
        let content_width = (content.0 * scale).round().clamp(1., f64::from(width)) as u32;
        let content_height = (content.1 * scale).round().clamp(1., f64::from(height)) as u32;

        Self {
            columns: columns as u32,
            rows: rows as u32,
            width,
            height,
            content: Rect {
                x: (width - content_width) / 2,
                y: (height - content_height) / 2,
                width: content_width,
                height: content_height,
            },
        }
    }

    /// Bytes of the RGBA texture.
    fn bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

/// Build the texture of a placement. Returns its RGBA pixels and whether they are all opaque.
fn render(image: &RgbaImage, source: Rect, layout: &Layout) -> (Vec<u8>, bool) {
    let cropped = if (source.x, source.y, source.width, source.height)
        == (0, 0, image.width(), image.height())
    {
        Cow::Borrowed(image)
    } else {
        Cow::Owned(
            imageops::crop_imm(image, source.x, source.y, source.width, source.height).to_image(),
        )
    };

    let content = resize(cropped, layout.content.width, layout.content.height);
    let texture = if content.dimensions() == (layout.width, layout.height) {
        content
    } else {
        let mut texture = RgbaImage::new(layout.width, layout.height);
        let (x, y) = (i64::from(layout.content.x), i64::from(layout.content.y));
        imageops::replace(&mut texture, &content, x, y);
        texture
    };

    let pixels = texture.into_raw();
    let is_opaque = pixels.chunks_exact(4).all(|pixel| pixel[3] == u8::MAX);
    (pixels, is_opaque)
}

/// Scale `image` to `width`x`height`.
fn resize(image: Cow<'_, RgbaImage>, width: u32, height: u32) -> RgbaImage {
    let source = image.dimensions();
    let shrunk = (cmp::min(source.0, width), cmp::min(source.1, height));
    let image = if shrunk == source {
        image
    } else {
        Cow::Owned(box_shrink(image.as_raw(), 4, source, shrunk.0, shrunk.1))
    };

    if image.dimensions() == (width, height) {
        image.into_owned()
    } else {
        enlarge(&image, width, height)
    }
}

/// Delete placements and images (`a=d`).
fn delete<L: EventListener>(term: &mut Term<L>, command: &Command) {
    // Any deletion may hide placements.
    term.graphics.kitty.damaged = true;

    // Uppercase forms also free the images left without placements.
    let free = command.delete.is_ascii_uppercase();
    let all = 0..usize::MAX;
    // Cell coordinates are 1-based; 0 matches nothing.
    let index = |position: u32| match position as usize {
        0 => 0..0,
        position => position - 1..position,
    };

    // Direct placements to delete, by the cells they cover or by z-index.
    let on_screen = |term: &Term<L>, lines: Range<usize>, columns: Range<usize>| {
        textures_in(term, lines, columns)
    };
    let cursor = term.grid().cursor.point;
    let textures = match command.delete.to_ascii_lowercase() {
        b'a' => Some((on_screen(term, all.clone(), all), None)),
        b'c' => {
            let (line, column) = (cursor.line.0 as usize, cursor.column.0);
            Some((on_screen(term, line..line + 1, column..column + 1), None))
        },
        b'p' => Some((on_screen(term, index(command.y), index(command.x)), None)),
        b'q' => Some((on_screen(term, index(command.y), index(command.x)), Some(command.z_index))),
        b'x' => Some((on_screen(term, all, index(command.x)), None)),
        b'y' => Some((on_screen(term, index(command.y), all), None)),
        b'z' => None,
        _ => {
            delete_images(term, command, free);
            return;
        },
    };

    let Graphics { kitty, texture_operations, .. } = &mut term.graphics;
    let images = match textures {
        Some((textures, z_index)) => kitty.delete_direct_placements(texture_operations, |p| {
            textures.contains(&p.graphic_id) && z_index.map_or(true, |z| p.z_index == z)
        }),
        None => {
            kitty.delete_direct_placements(texture_operations, |p| p.z_index == command.z_index)
        },
    };

    if free {
        for id in images {
            if !kitty.has_placements(id) {
                kitty.free_image(id);
            }
        }

        if command.delete == b'A' {
            kitty.free_unplaced_images();
        }
    }
}

/// Delete placements of images selected by id (`d=i`, `d=n`, `d=r`), virtual ones included.
fn delete_images<L: EventListener>(term: &mut Term<L>, command: &Command, free: bool) {
    let Graphics { kitty, texture_operations, .. } = &mut term.graphics;

    let (images, placement_id): (Vec<u32>, u32) = match command.delete.to_ascii_lowercase() {
        b'i' if command.image_id != 0 => (vec![command.image_id], command.placement_id),
        b'n' if command.image_number != 0 => (
            kitty.newest_with_number(command.image_number).into_iter().collect(),
            command.placement_id,
        ),
        b'r' => {
            let range = command.x..=command.y;
            let mut ids: HashSet<u32> =
                kitty.images.keys().copied().filter(|id| range.contains(id)).collect();
            ids.extend(
                kitty.virtual_placements.keys().map(|&(id, _)| id).filter(|id| range.contains(id)),
            );
            ids.extend(
                kitty.direct_placements.keys().map(|&(id, ..)| id).filter(|id| range.contains(id)),
            );
            (ids.into_iter().filter(|&id| id != 0).collect(), 0)
        },
        b'f' => return,
        _ => {
            log::debug!("Ignoring kitty graphics deletion d={:?}", command.delete as char);
            return;
        },
    };

    for id in images {
        kitty.delete_image_placements(texture_operations, id, placement_id);
        if free && !kitty.has_placements(id) {
            kitty.free_image(id);
        }
    }
}

/// Graphics referenced by the cells of the screen in `lines × columns` (clipped to the screen).
fn textures_in<L>(
    term: &Term<L>,
    lines: Range<usize>,
    columns: Range<usize>,
) -> HashSet<GraphicId> {
    let grid = term.grid();
    let lines = lines.start..cmp::min(lines.end, grid.screen_lines());
    let columns = columns.start..cmp::min(columns.end, grid.columns());

    let mut textures = HashSet::new();
    for line in lines {
        let row = &grid[Line(line as i32)];
        for column in columns.clone() {
            if let Some(graphics) = row[Column(column)].graphics() {
                textures.extend(graphics.iter().map(|graphic| graphic.texture.id));
            }
        }
    }

    textures
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;
    use std::time::{Duration, Instant};

    use image::{ImageFormat, Rgba};

    use crate::event::Event;
    use crate::index::Point;
    use crate::term::{Config, TermDamage};
    use crate::vte::ansi;

    /// Terminal size with pixel dimensions.
    struct PixelSize {
        cell: (f32, f32),
    }

    impl Dimensions for PixelSize {
        fn total_lines(&self) -> usize {
            24
        }

        fn screen_lines(&self) -> usize {
            24
        }

        fn columns(&self) -> usize {
            80
        }

        fn cell_width(&self) -> f32 {
            self.cell.0
        }

        fn cell_height(&self) -> f32 {
            self.cell.1
        }
    }

    /// Collects the replies written to the PTY.
    #[derive(Clone, Default)]
    struct Replies(Arc<Mutex<Vec<String>>>);

    impl Replies {
        fn take(&self) -> Vec<String> {
            mem::take(&mut *self.0.lock())
        }
    }

    impl EventListener for Replies {
        fn send_event(&self, event: Event) {
            if let Event::PtyWrite(text) = event {
                self.0.lock().push(text);
            }
        }
    }

    /// 80x24 terminal with 10x20 pixel cells.
    fn term() -> (Term<Replies>, Replies) {
        let replies = Replies::default();
        let size = PixelSize { cell: (10., 20.) };
        (Term::new(Config::default(), &size, replies.clone()), replies)
    }

    fn feed(term: &mut Term<Replies>, bytes: &[u8]) {
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(term, bytes);
    }

    /// Send a graphics command.
    fn command(term: &mut Term<Replies>, control: &str, payload: &str) {
        feed(term, format!("\x1b_G{control};{payload}\x1b\\").as_bytes());
    }

    /// Opaque PNG whose pixel `(x, y)` is `(x, y, 0x80)`.
    fn png(width: u32, height: u32) -> String {
        let image = RgbaImage::from_fn(width, height, |x, y| Rgba([x as u8, y as u8, 0x80, 0xff]));
        let mut file = Cursor::new(Vec::new());
        image.write_to(&mut file, ImageFormat::Png).unwrap();
        BASE64.encode(file.into_inner())
    }

    fn ok(keys: &str) -> String {
        format!("\x1b_G{keys};OK\x1b\\")
    }

    /// Screen cells holding graphics, as `(line, column)`.
    fn graphic_cells(term: &Term<Replies>) -> Vec<(i32, usize)> {
        let grid = term.grid();
        (0..grid.screen_lines() as i32)
            .flat_map(|line| (0..grid.columns()).map(move |column| (line, column)))
            .filter(|&(line, column)| grid[Line(line)][Column(column)].graphics().is_some())
            .collect()
    }

    fn cells(lines: Range<i32>, columns: Range<usize>) -> Vec<(i32, usize)> {
        lines.flat_map(|line| columns.clone().map(move |column| (line, column))).collect()
    }

    fn stored(term: &Term<Replies>) -> Vec<u32> {
        let mut ids: Vec<u32> = term.graphics.kitty.images.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Textures uploaded and removed since the last call. Kitty textures are all retained.
    fn queues(term: &mut Term<Replies>) -> (Vec<GraphicData>, Vec<GraphicId>) {
        let queues = term.graphics_take_queues().unwrap_or_default();
        assert!(queues.pending.is_empty(), "kitty textures must use the retained queue");
        (queues.retained, queues.remove_queue)
    }

    /// Whether the screen is fully damaged; resets the damage.
    fn fully_damaged(term: &mut Term<Replies>) -> bool {
        let full = matches!(term.damage(), TermDamage::Full);
        term.reset_damage();
        full
    }

    #[test]
    fn parse_control_data() {
        let command = Command::parse(
            b"a=T,f=100,q=2,i=7,I=0,p=3,c=10,r=5,C=1,U=1,z=-4,m=1,x=1,y=2,w=3,h=4,s=5,v=6,t=d,\
              d=I,P=0,Q=0,H=-1,V=2,N=1,S=10,O=0,X=1,Y=2,k=7",
        )
        .unwrap();

        assert_eq!((command.action, command.format, command.quiet), (b'T', 100, 2));
        assert_eq!((command.image_id, command.image_number, command.placement_id), (7, 0, 3));
        assert_eq!((command.columns, command.rows, command.z_index), (10, 5, -4));
        assert!(command.keep_cursor && command.unicode && command.more && !command.relative);
        assert_eq!(
            (command.x, command.y, command.source_width, command.source_height),
            (1, 2, 3, 4)
        );
        assert_eq!(
            (command.width, command.height, command.medium, command.delete),
            (5, 6, b'd', b'I')
        );
        assert!(command.keys.contains(b'k') && command.keys.contains(b'X'));
        assert!(!command.keys.contains(b'o'));

        // Defaults, empty pairs, clamped `q`, relative placements.
        assert_eq!(Command::parse(b"").unwrap(), Command::default());
        let command = Command::parse(b",i=1,,q=5,P=2,").unwrap();
        assert_eq!((command.image_id, command.quiet, command.relative), (1, 2, true));
        assert_eq!((command.action, command.format, command.delete), (b't', 32, b'a'));

        for invalid in [
            &b"i=x"[..],
            b"i",
            b"i=",
            b"a=TT",
            b"ab=1",
            b"=1",
            b"i=-1",
            b"i=4294967296",
            b"z=1.5",
            b"m=1;",
        ] {
            assert!(Command::parse(invalid).is_err(), "{}", String::from_utf8_lossy(invalid));
        }

        // Continuation chunks.
        let first = Command::parse(b"a=t,f=100,i=1,m=1").unwrap();
        let continues = |control: &[u8]| Command::parse(control).unwrap().continues(&first);
        assert!(continues(b"m=1"));
        assert!(continues(b"q=2,m=0"));
        assert!(continues(b"a=t,m=0"));
        assert!(continues(b""));
        assert!(!continues(b"a=T,m=0"));
        assert!(!continues(b"i=31,s=1,v=1,a=q,t=d,f=24"));
        assert!(!continues(b"f=100,i=2,m=1"));
    }

    #[test]
    fn query_reply() {
        let (mut term, replies) = term();

        // The probe must be answered before the device attributes that follow it.
        feed(&mut term, b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c");
        assert_eq!(replies.take(), ["\x1b_Gi=31;OK\x1b\\", "\x1b[?62;4;6;22c"]);
        assert!(stored(&term).is_empty());
        assert!(term.graphics_take_queues().is_none());

        // `q=1` suppresses OK, `q=2` errors too; no reply without an id.
        command(&mut term, "i=31,s=1,v=1,a=q,f=24,q=1", "AAAA");
        command(&mut term, "i=33,s=2,v=1,a=q,f=24,q=2", "AAAA");
        command(&mut term, "s=2,v=1,a=q,f=24", "AAAA");
        assert!(replies.take().is_empty());

        command(&mut term, "i=32,s=2,v=1,a=q,f=24,q=1", "AAAA");
        command(&mut term, "i=34,a=q,t=f", "L3RtcC94");
        command(&mut term, "i=35,a=q,f=100", &BASE64.encode("not an image"));
        command(&mut term, "i=36,a=q,f=24,s=1,v=1,o=z", "AAAA");
        command(&mut term, "i=37,a=q,f=24,v=1", "AAAA");
        command(&mut term, "I=38,a=q,f=99,s=1,v=1", "AAAA");
        let replies = replies.take();
        assert_eq!(
            replies[..2],
            [
                "\x1b_Gi=32;ENODATA:insufficient image data\x1b\\",
                "\x1b_Gi=34;EINVAL:only direct transmission (t=d) is supported\x1b\\",
            ]
        );
        assert!(replies[2].starts_with("\x1b_Gi=35;EBADPNG:"), "{:?}", replies[2]);
        assert!(replies[2].bytes().all(|byte| byte == 0x1b || (0x20..0x7f).contains(&byte)));
        assert_eq!(
            replies[3..],
            [
                "\x1b_Gi=36;EINVAL:compression is not supported\x1b\\",
                "\x1b_Gi=37;EINVAL:missing image size\x1b\\",
                "\x1b_GI=38;EINVAL:unknown format\x1b\\",
            ]
        );
    }

    #[test]
    fn transmit_formats_and_ids() {
        let (mut term, replies) = term();

        command(&mut term, "a=t,f=100,i=5", &png(3, 2));
        command(&mut term, "a=t,f=24,s=2,v=1,i=6", &BASE64.encode([1, 2, 3, 4, 5, 6]));
        command(&mut term, "a=t,s=1,v=1,i=7,p=2", &BASE64.encode([9, 8, 7, 6, 0xff]));
        assert_eq!(replies.take(), [ok("i=5"), ok("i=6"), ok("i=7,p=2")]);

        let images = &term.graphics.kitty.images;
        assert_eq!(images[&5].pixels.dimensions(), (3, 2));
        assert_eq!(images[&5].pixels.get_pixel(2, 1).0, [2, 1, 0x80, 0xff]);
        assert_eq!(images[&6].pixels.as_raw(), &[1, 2, 3, 0xff, 4, 5, 6, 0xff]);
        assert_eq!(images[&7].pixels.as_raw(), &[9, 8, 7, 6]);
        assert_eq!(term.graphics.kitty.stored_bytes, 3 * 2 * 4 + 2 * 4 + 4);

        // Transmitting is not displaying.
        assert!(graphic_cells(&term).is_empty());
        assert!(term.graphics_take_queues().is_none());

        // Without an id the image can't be referenced: not stored, no reply.
        command(&mut term, "a=t,f=24,s=1,v=1", "AAAA");
        assert_eq!(stored(&term), [5, 6, 7]);

        // Image numbers get terminal-assigned ids; placements use the newest.
        command(&mut term, "a=t,f=24,s=1,v=1,I=13", "AAAA");
        command(&mut term, "a=t,f=24,s=1,v=1,I=13", "AAAA");
        command(&mut term, "a=p,I=13,U=1,c=1,r=1", "");
        command(&mut term, "a=p,I=14", "");
        command(&mut term, "a=t,f=24,s=1,v=1,i=1,I=2", "AAAA");
        command(&mut term, "a=p", "");
        assert_eq!(
            replies.take(),
            [
                ok("i=2147483648,I=13"),
                ok("i=2147483649,I=13"),
                ok("i=2147483649,I=13"),
                "\x1b_GI=14;ENOENT:image not found\x1b\\".into(),
                "\x1b_Gi=1,I=2;EINVAL:both i and I given\x1b\\".into(),
            ]
        );
        assert!(term.kitty_virtual_placement(2147483649, 0).is_some());

        // Retransmission replaces the image and deletes its placements.
        command(&mut term, "a=t,f=24,s=1,v=1,I=13,i=2147483649,q=2", "AAAA");
        command(&mut term, "a=t,f=24,s=1,v=1,i=2147483649,q=2", "AAAA");
        assert!(term.kitty_virtual_placement(2147483649, 0).is_none());
    }

    #[test]
    fn chunked_transfer_with_interleaved_query() {
        let data = png(30, 20);

        // Chunk sizes that are and are not multiples of 4.
        for size in [4096, 8, 7, 1] {
            let (mut term, replies) = term();
            let chunks: Vec<&str> =
                data.as_bytes().chunks(size).map(|chunk| str::from_utf8(chunk).unwrap()).collect();

            command(&mut term, "a=t,f=100,i=5,q=1,m=1", "");
            for (index, chunk) in chunks.iter().enumerate() {
                if index == 1 {
                    command(&mut term, "i=31,s=1,v=1,a=q,t=d,f=24", "AAAA");
                }

                let more = if index + 1 < chunks.len() { 1 } else { 0 };
                // The last chunk may change `q`.
                let quiet = if more == 1 { "" } else { "q=0," };
                command(&mut term, &format!("{quiet}m={more}"), chunk);
            }

            let expected =
                if chunks.len() > 1 { vec![ok("i=31"), ok("i=5")] } else { vec![ok("i=5")] };
            assert_eq!(replies.take(), expected, "chunk size {size}");
            assert_eq!(term.graphics.kitty.images[&5].pixels.dimensions(), (30, 20));
            assert!(term.graphics.kitty.transfer.is_none());
        }
    }

    #[test]
    fn apc_string_split_across_reads() {
        let (mut term, replies) = term();
        let sequence = format!("\x1b_Ga=t,f=100,i=5;{}\x1b\\", png(4, 4));
        let (first, second) = sequence.as_bytes().split_at(10);

        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, first);
        assert!(stored(&term).is_empty());
        parser.advance(&mut term, second);

        assert_eq!(replies.take(), [ok("i=5")]);
        assert_eq!(stored(&term), [5]);
    }

    #[test]
    fn abandoned_and_failed_transfers() {
        let (mut term, replies) = term();
        let data = png(8, 8);
        let (first, rest) = data.split_at(8);

        // A new transmission replaces an incomplete one.
        command(&mut term, "a=t,f=100,i=1,m=1", first);
        command(&mut term, "a=t,f=100,i=2", &data);
        assert_eq!(replies.take(), [ok("i=2")]);
        assert_eq!(stored(&term), [2]);

        // A stray last chunk is a new transmission of default format, without id.
        command(&mut term, "m=0", rest);
        assert!(replies.take().is_empty());

        // A deletion aborts the transmission.
        command(&mut term, "a=t,f=100,i=3,m=1", first);
        command(&mut term, "a=d,d=i,i=99", "");
        command(&mut term, "m=0", rest);
        assert!(replies.take().is_empty());
        assert_eq!(stored(&term), [2]);

        // A transmission that fails is reported once, at its end.
        command(&mut term, "a=t,f=100,i=4,m=1", "****");
        command(&mut term, "m=1", "AAAA");
        command(&mut term, "m=0", "AAAA");
        assert_eq!(replies.take(), ["\x1b_Gi=4;EINVAL:invalid base64 data\x1b\\"]);
        assert!(term.graphics.kitty.transfer.is_none());
    }

    #[test]
    fn ignores_other_apc_strings() {
        let (mut term, replies) = term();
        feed(&mut term, b"\x1b_Xi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b_\x1b\\\x1b_gi=1;\x1b\\");
        assert!(replies.take().is_empty());

        feed(&mut term, b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\");
        assert_eq!(replies.take(), [ok("i=31")]);
    }

    #[test]
    fn layout_fits_cells() {
        let rect = |x, y, width, height| Rect { x, y, width, height };

        // Virtual: fitted into the cells, centred.
        let layout = Layout::new((40, 20), (8, 5), (10, 20), true);
        assert_eq!(
            layout,
            Layout { columns: 8, rows: 5, width: 80, height: 100, content: rect(0, 30, 80, 40) }
        );

        // Virtual without size: the cells covered by the image, filled.
        let layout = Layout::new((25, 30), (0, 0), (10, 20), true);
        assert_eq!(
            layout,
            Layout { columns: 3, rows: 2, width: 30, height: 40, content: rect(0, 2, 30, 36) }
        );

        // Direct: native size, or the other dimension follows the aspect ratio.
        let layout = Layout::new((25, 30), (0, 0), (10, 20), false);
        assert_eq!((layout.columns, layout.rows, layout.width, layout.height), (3, 2, 25, 30));
        assert_eq!(layout.content, rect(0, 0, 25, 30));
        let layout = Layout::new((40, 20), (4, 0), (10, 20), false);
        assert_eq!((layout.columns, layout.rows, layout.width, layout.height), (4, 1, 40, 20));
        let layout = Layout::new((40, 20), (0, 3), (10, 20), false);
        assert_eq!((layout.columns, layout.rows, layout.width, layout.height), (12, 3, 120, 60));

        // Direct with both: letterboxed.
        let layout = Layout::new((40, 20), (2, 2), (10, 20), false);
        assert_eq!((layout.width, layout.height), (20, 40));
        assert_eq!(layout.content, rect(0, 15, 20, 10));

        // Clamped to the texture limits.
        let layout = Layout::new((100, 100), (1000, 0), (10, 20), false);
        assert_eq!(
            (layout.columns, layout.rows, layout.width, layout.height),
            (1000, 500, 4096, 4096)
        );
        let layout = Layout::new((100, 100), (1000, 1000), (10, 20), true);
        assert_eq!((layout.width, layout.height), (2048, 4096));
        assert_eq!(layout.content, rect(0, 1024, 2048, 2048));
    }

    #[test]
    fn virtual_placement() {
        let (mut term, replies) = term();
        feed(&mut term, b"\x1b[3;5H");
        let cursor = term.grid().cursor.point;

        command(&mut term, "a=t,f=100,i=9,q=2", &png(40, 20));
        // Placed after the transmission: uploaded at placement.
        assert!(term.graphics_take_queues().is_none());
        command(&mut term, "a=p,U=1,i=9,c=8,r=5", "");
        assert_eq!(replies.take(), [ok("i=9")]);

        let placement = term.kitty_virtual_placement(9, 0).expect("virtual placement");
        assert_eq!((placement.columns, placement.rows), (8, 5));
        assert_eq!((placement.tile_width, placement.tile_height), (10., 20.));
        let texture = placement.texture.clone();
        assert_eq!((texture.width, texture.height, texture.cell_height), (80, 100, 20));
        assert!(term.kitty_virtual_placement(9, 1).is_none());
        assert!(term.kitty_virtual_placement(8, 0).is_none());

        // Nothing is written to the grid; the cursor stays.
        assert!(graphic_cells(&term).is_empty());
        assert_eq!(term.grid().cursor.point, cursor);

        // The image is centred with transparent padding.
        let (pending, _) = queues(&mut term);
        assert_eq!(pending.len(), 1);
        let graphic = &pending[0];
        assert_eq!((graphic.id, graphic.width, graphic.height), (texture.id, 80, 100));
        assert!(!graphic.is_opaque);
        let alpha = |x: usize, y: usize| graphic.pixels[(y * 80 + x) * 4 + 3];
        assert_eq!(
            (alpha(0, 0), alpha(40, 29), alpha(40, 30), alpha(79, 69), alpha(40, 70)),
            (0, 0, 255, 255, 0)
        );

        // `a=T,U=1` creates a virtual placement too, with or without placement id.
        command(&mut term, "a=T,U=1,f=100,i=10,p=4,c=2,r=1,q=2", &png(20, 20));
        command(&mut term, "a=T,U=1,f=100,c=2,r=1", &png(20, 20));
        assert!(replies.take().is_empty());
        let placement = term.kitty_virtual_placement(10, 0).expect("virtual placement");
        assert_eq!((placement.columns, placement.rows), (2, 1));
        assert!(term.kitty_virtual_placement(10, 4).is_some());
        assert!(graphic_cells(&term).is_empty());
        assert_eq!(term.grid().cursor.point, cursor);

        // The lowest placement id answers placement id 0.
        command(&mut term, "a=p,U=1,i=10,p=2,c=3,r=1,q=2", "");
        assert_eq!(term.kitty_virtual_placement(10, 0).unwrap().columns, 3);
    }

    #[test]
    fn direct_placement_and_cursor() {
        let (mut term, replies) = term();
        command(&mut term, "a=t,f=100,i=9,q=2", &png(40, 40));

        // `C=1` keeps the cursor.
        feed(&mut term, b"\x1b[6;4H");
        command(&mut term, "a=p,i=9,p=1,c=4,r=2,C=1", "");
        assert_eq!(replies.take(), [ok("i=9,p=1")]);
        assert_eq!(graphic_cells(&term), cells(5..7, 3..7));
        assert_eq!(term.grid().cursor.point, Point::new(Line(5), Column(3)));
        let (pending, _) = queues(&mut term);
        assert_eq!((pending[0].width, pending[0].height), (40, 40));
        assert!(pending[0].is_opaque);

        // Otherwise the cursor ends on the last row, after the image.
        feed(&mut term, b"\x1b[2J\x1b[1;1H");
        command(&mut term, "a=p,i=9,c=4,r=2,q=2", "");
        assert_eq!(graphic_cells(&term), cells(0..2, 0..4));
        assert_eq!(term.grid().cursor.point, Point::new(Line(1), Column(4)));

        // `a=T` without id, native size (3x2 cells).
        feed(&mut term, b"\x1b[2J\x1b[11;11H");
        command(&mut term, "a=T,f=100", &png(25, 30));
        assert!(replies.take().is_empty());
        assert_eq!(graphic_cells(&term), cells(10..12, 10..13));
        assert_eq!(term.grid().cursor.point, Point::new(Line(11), Column(13)));

        // At the bottom, the screen scrolls; with `C=1` the image is clipped instead.
        feed(&mut term, b"\x1b[2J\x1b[24;1H");
        command(&mut term, "a=p,i=9,c=2,r=3", "");
        assert_eq!(graphic_cells(&term), cells(21..24, 0..2));
        assert_eq!(term.grid().cursor.point, Point::new(Line(23), Column(2)));
        feed(&mut term, b"\x1b[2J\x1b[24;1H");
        command(&mut term, "a=p,i=9,c=2,r=3,C=1", "");
        assert_eq!(graphic_cells(&term), cells(23..24, 0..2));
        assert_eq!(term.grid().cursor.point, Point::new(Line(23), Column(0)));

        // The cursor stops at the last column.
        feed(&mut term, b"\x1b[2J\x1b[1;79H");
        command(&mut term, "a=p,i=9,c=4,r=1", "");
        assert_eq!(graphic_cells(&term), cells(0..1, 78..80));
        assert_eq!(term.grid().cursor.point, Point::new(Line(0), Column(79)));
    }

    #[test]
    fn crop_source_rectangle() {
        let (mut term, replies) = term();
        command(&mut term, "a=t,f=100,i=9,q=2", &png(40, 40));
        queues(&mut term);

        command(&mut term, "a=p,i=9,x=5,y=10,w=30,h=20,C=1", "");
        let (pending, _) = queues(&mut term);
        let graphic = &pending[0];
        assert_eq!((graphic.width, graphic.height), (30, 20));
        assert_eq!(graphic.pixels[..4], [5, 10, 0x80, 0xff]);
        assert_eq!(graphic_cells(&term), cells(0..1, 0..3));

        // Clipped to the image; an empty rectangle is an error.
        command(&mut term, "a=p,i=9,x=30,y=35,w=100,C=1", "");
        let (pending, _) = queues(&mut term);
        assert_eq!((pending[0].width, pending[0].height), (10, 5));
        command(&mut term, "a=p,i=9,x=40", "");
        assert_eq!(
            replies.take(),
            [ok("i=9"), ok("i=9"), "\x1b_Gi=9;EINVAL:empty source rectangle\x1b\\".into()]
        );
    }

    #[test]
    fn replacing_a_placement_removes_its_texture() {
        let (mut term, _) = term();
        command(&mut term, "a=t,f=100,i=9,q=2", &png(20, 20));

        command(&mut term, "a=p,i=9,p=1,C=1,q=2", "");
        let (pending, _) = queues(&mut term);
        let first = pending[0].id;

        // Same ids: the old texture is removed, even with its cells still on the grid.
        feed(&mut term, b"\x1b[5;5H");
        command(&mut term, "a=p,i=9,p=1,C=1,q=2", "");
        let (pending, removed) = queues(&mut term);
        assert_eq!(removed, [first]);
        let second = pending[0].id;

        // Another placement id adds a placement.
        command(&mut term, "a=p,i=9,p=2,C=1,q=2", "");
        let (_, removed) = queues(&mut term);
        assert!(removed.is_empty());

        // A placement replaced before it was uploaded is never uploaded.
        command(&mut term, "a=p,i=9,p=2,U=1,c=1,r=1,q=2", "");
        command(&mut term, "a=p,i=9,p=2,U=1,c=2,r=1,q=2", "");
        let (pending, removed) = queues(&mut term);
        assert_eq!(pending.len(), 1);
        assert_eq!((pending[0].width, pending[0].height), (20, 20));
        assert_eq!(term.graphics.kitty.direct_placements.len(), 1);
        assert!(removed.len() == 1 && removed[0] != second);

        // A direct placement replaces a virtual one with the same ids.
        command(&mut term, "a=p,i=9,p=2,C=1,q=2", "");
        assert!(term.kitty_virtual_placement(9, 2).is_none());
    }

    #[test]
    fn deletions() {
        let (mut term, replies) = term();
        command(&mut term, "a=t,f=100,i=1,q=2", &png(10, 20));
        command(&mut term, "a=t,f=100,i=2,q=2", &png(10, 20));
        command(&mut term, "a=p,U=1,i=1,p=1,q=2", "");
        command(&mut term, "a=p,U=1,i=1,p=2,q=2", "");
        command(&mut term, "a=p,i=2,p=1,C=1,q=2", "");
        queues(&mut term);
        let direct = term.graphics.kitty.direct_placements.values().next().unwrap().graphic_id;

        // One placement; the image stays.
        command(&mut term, "a=d,d=i,i=1,p=1", "");
        assert!(term.kitty_virtual_placement(1, 1).is_none());
        assert!(term.kitty_virtual_placement(1, 2).is_some());
        queues(&mut term);

        // Visible placements: virtual placements are not affected.
        command(&mut term, "a=d,d=a", "");
        assert_eq!(queues(&mut term).1, [direct]);
        assert!(term.kitty_virtual_placement(1, 2).is_some());
        assert_eq!(stored(&term), [1, 2]);

        // Uppercase also frees the images left without placements.
        command(&mut term, "a=d,d=A", "");
        assert_eq!(stored(&term), [1]);
        command(&mut term, "a=p,i=2", "");
        assert_eq!(replies.take(), ["\x1b_Gi=2;ENOENT:image not found\x1b\\"]);

        // Lowercase keeps the data, uppercase frees it.
        command(&mut term, "a=d,d=i,i=1", "");
        assert!(term.kitty_virtual_placement(1, 2).is_none());
        command(&mut term, "a=p,U=1,i=1,q=2", "");
        assert!(term.kitty_virtual_placement(1, 0).is_some());
        command(&mut term, "a=d,d=I,i=1", "");
        assert!(term.kitty_virtual_placement(1, 0).is_none());
        assert!(stored(&term).is_empty());
        assert_eq!(term.graphics.kitty.stored_bytes, 0);

        // Image numbers: only the newest image.
        command(&mut term, "a=t,f=24,s=1,v=1,I=4,q=2", "AAAA");
        command(&mut term, "a=t,f=24,s=1,v=1,I=4,q=2", "AAAA");
        command(&mut term, "a=d,d=N,I=4", "");
        assert_eq!(stored(&term), [2147483648]);

        // Id ranges.
        for id in 5..=7 {
            command(&mut term, &format!("a=t,f=24,s=1,v=1,i={id},q=2"), "AAAA");
            command(&mut term, &format!("a=p,U=1,i={id},q=2"), "");
        }
        command(&mut term, "a=d,d=R,x=6,y=7", "");
        assert_eq!(stored(&term), [5, 2147483648]);
        assert!(term.kitty_virtual_placement(5, 0).is_some());
        assert!(term.kitty_virtual_placement(6, 0).is_none());
    }

    #[test]
    fn deletions_by_position() {
        let (mut term, _) = term();
        command(&mut term, "a=t,f=100,i=1,q=2", &png(20, 20));
        let place = |term: &mut Term<Replies>, z: i32| {
            // Two cells at line 3, columns 5 and 6 (1-based).
            feed(term, b"\x1b[3;5H");
            command(term, &format!("a=p,i=1,C=1,z={z},q=2"), "");
        };

        for (delete, deleted) in [
            ("d=p,x=4,y=3", false),
            ("d=p,x=6,y=3", true),
            ("d=q,x=5,y=3,z=1", false),
            ("d=q,x=5,y=3,z=-1", true),
            ("d=x,x=6", true),
            ("d=x,x=7", false),
            ("d=y,y=3", true),
            ("d=y,y=4", false),
            ("d=z,z=-1", true),
            ("d=z,z=0", false),
        ] {
            place(&mut term, -1);
            queues(&mut term);
            command(&mut term, &format!("a=d,{delete}"), "");
            assert_eq!(!queues(&mut term).1.is_empty(), deleted, "{delete}");
            // Free the rows, so the cells of placements that were not deleted are released.
            feed(&mut term, b"\x1b[2J\x1b[3J");
        }

        // At the cursor.
        place(&mut term, 0);
        queues(&mut term);
        command(&mut term, "a=d,d=c", "");
        assert!(!queues(&mut term).1.is_empty());
        command(&mut term, "a=d,d=C", "");
        assert_eq!(stored(&term), [1]);
        place(&mut term, 0);
        command(&mut term, "a=d,d=C", "");
        assert!(stored(&term).is_empty());
    }

    #[test]
    fn deleted_graphics_are_removed_harmlessly() {
        let (mut term, _) = term();
        command(&mut term, "a=t,f=100,i=1,q=2", &png(20, 20));

        // Deleted before the upload: never uploaded.
        command(&mut term, "a=p,i=1,C=1,q=2", "");
        let never_uploaded =
            term.graphics.kitty.direct_placements.values().next().unwrap().graphic_id;
        command(&mut term, "a=d,d=i,i=1", "");
        assert_eq!(queues(&mut term), (Vec::new(), Vec::new()));

        // Deleted after the upload: the texture is removed.
        command(&mut term, "a=p,i=1,C=1,q=2", "");
        let (pending, _) = queues(&mut term);
        let uploaded = pending[0].id;
        command(&mut term, "a=d,d=i,i=1", "");
        assert_eq!(queues(&mut term).1, [uploaded]);

        // The cells still on the grid remove both textures again when they are dropped. The
        // display ignores removals of unknown textures (`GraphicsRenderer::remove_graphics`).
        assert_eq!(graphic_cells(&term).len(), 2);
        feed(&mut term, b"\x1b[2J\x1b[3J");
        let mut removed = queues(&mut term).1;
        removed.sort_unstable();
        assert_eq!(removed, [never_uploaded, uploaded]);
        assert!(graphic_cells(&term).is_empty());
    }

    #[test]
    fn quota_evicts_oldest_unreferenced_images() {
        let (mut term, _) = term();
        // Room for three 2x2 images.
        term.graphics.kitty.storage_quota = 3 * 16;
        let transmit = |term: &mut Term<Replies>, id: u32| {
            command(term, &format!("a=t,s=2,v=2,i={id},q=2"), &BASE64.encode([0xff; 16]));
        };

        for id in 1..=3 {
            transmit(&mut term, id);
        }
        command(&mut term, "a=p,U=1,i=1,q=2", "");
        transmit(&mut term, 4);
        assert_eq!(stored(&term), [1, 3, 4]);
        transmit(&mut term, 5);
        assert_eq!(stored(&term), [1, 4, 5]);

        // With every image placed, the oldest goes, with its placements.
        command(&mut term, "a=p,U=1,i=4,q=2", "");
        command(&mut term, "a=p,i=5,C=1,q=2", "");
        transmit(&mut term, 6);
        assert_eq!(stored(&term), [4, 5, 6]);
        assert!(term.kitty_virtual_placement(1, 0).is_none());
        assert_eq!(term.graphics.kitty.stored_bytes, 3 * 16);

        // Retransmission of a stored id does not evict others.
        transmit(&mut term, 6);
        assert_eq!(stored(&term), [4, 5, 6]);

        // The image limit counts images, whatever their size.
        term.graphics.kitty.image_limit = 2;
        transmit(&mut term, 7);
        assert_eq!(stored(&term), [5, 7]);
    }

    #[test]
    fn reset_and_missing_cell_size() {
        let (mut term, replies) = term();
        command(&mut term, "a=T,U=1,f=100,i=1,q=2", &png(4, 4));
        feed(&mut term, b"\x1bc");
        assert!(term.kitty_virtual_placement(1, 0).is_none());
        assert!(stored(&term).is_empty());

        // Without cell size the image is stored but can't be placed.
        let mut term = Term::new(Config::default(), &PixelSize { cell: (0., 0.) }, replies.clone());
        command(&mut term, "a=T,f=100,i=1", &png(4, 4));
        assert_eq!(replies.take(), ["\x1b_Gi=1;EINVAL:the terminal has no cell size\x1b\\"]);
        assert_eq!(stored(&term), [1]);
    }

    #[test]
    fn placement_changes_damage_the_screen() {
        let (mut term, _) = term();
        // The cursor is far from the placeholder cells, which are not rewritten.
        feed(&mut term, b"\x1b[20;70H");
        fully_damaged(&mut term);

        let image = png(20, 20);
        command(&mut term, "a=t,f=100,i=9,q=2", &image);
        assert!(!fully_damaged(&mut term), "transmission");

        for (control, payload, change) in [
            ("a=p,U=1,i=9,p=1,c=2,r=1", "", "new virtual placement"),
            ("a=p,U=1,i=9,p=1,c=3,r=1", "", "replaced virtual placement"),
            ("a=p,U=1,i=9,p=1,c=3,r=1", "", "identical virtual placement"),
            ("a=d,d=i,i=9,p=1", "", "deleted virtual placement"),
            ("a=p,i=9,p=2,C=1", "", "direct placement"),
            ("a=d,d=a", "", "deleted direct placement"),
            ("a=p,U=1,i=9,c=2,r=1", "", "virtual placement without placement id"),
            ("a=t,f=100,i=9", image.as_str(), "retransmission of a placed image"),
            ("a=d,d=I,i=9", "", "deleted image"),
        ] {
            command(&mut term, &format!("{control},q=2"), payload);
            assert!(fully_damaged(&mut term), "{change}");
        }

        // Quota eviction.
        command(&mut term, "a=T,U=1,f=24,s=1,v=1,i=1,q=2", "AAAA");
        fully_damaged(&mut term);
        term.graphics.kitty.storage_quota = 4;
        command(&mut term, "a=t,f=24,s=1,v=1,i=2,q=2", "AAAA");
        assert_eq!(stored(&term), [2]);
        assert!(fully_damaged(&mut term), "evicted image");
    }

    #[test]
    fn chunked_query_is_answered_once() {
        let (mut term, replies) = term();

        // Six bytes of RGB data in two chunks.
        command(&mut term, "a=q,i=31,f=24,s=2,v=1,t=d,m=1", "AQID");
        assert!(replies.take().is_empty());
        command(&mut term, "m=0", "BAUG");
        assert_eq!(replies.take(), [ok("i=31")]);
        assert!(stored(&term).is_empty());
        assert!(term.graphics_take_queues().is_none());
        assert!(term.graphics.kitty.query.is_none());

        // Inside a chunked transmission, the query takes the continuation chunks until it
        // ends; then the transmission resumes.
        let data = png(8, 8);
        let (first, rest) = data.split_at(8);
        command(&mut term, "a=t,f=100,i=5,m=1", first);
        command(&mut term, "a=q,i=32,f=24,s=2,v=1,m=1", "AQID");
        command(&mut term, "q=0,m=0", "BAUG");
        command(&mut term, "m=0", rest);
        assert_eq!(replies.take(), [ok("i=32"), ok("i=5")]);
        assert_eq!(stored(&term), [5]);

        // Missing data is reported once, at the end; `q` of the last chunk applies.
        command(&mut term, "a=q,i=33,f=24,s=2,v=1,m=1", "AQID");
        command(&mut term, "m=0", "");
        command(&mut term, "a=q,i=34,f=24,s=2,v=1,m=1", "AQID");
        command(&mut term, "q=2,m=0", "");
        assert_eq!(replies.take(), ["\x1b_Gi=33;ENODATA:insufficient image data\x1b\\"]);

        // Deleting aborts a query open inside a transmission as well as the transmission.
        command(&mut term, "a=t,i=6,f=24,s=2,v=1,m=1", "AQID");
        command(&mut term, "a=q,i=35,f=24,s=2,v=1,m=1", "AQID");
        command(&mut term, "a=d,d=a", "");
        assert!(term.graphics.kitty.transfer.is_none());
        assert!(term.graphics.kitty.query.is_none());
        command(&mut term, "m=0", "BAUG");
        assert!(replies.take().is_empty());
    }

    #[test]
    fn both_ids_are_rejected_before_any_action() {
        let (mut term, replies) = term();
        command(&mut term, "a=t,f=24,s=1,v=1,i=31,q=2", "AAAA");
        command(&mut term, "a=p,U=1,i=31,q=2", "");
        queues(&mut term);

        command(&mut term, "a=d,d=I,i=31,I=13", "");
        command(&mut term, "a=q,i=31,I=13,f=24,s=1,v=1", "AAAA");
        command(&mut term, "a=p,U=1,i=31,I=13,p=2", "");
        command(&mut term, "a=t,f=24,s=1,v=1,i=31,I=13", "AAAA");
        // `q=1` only suppresses success.
        command(&mut term, "a=d,d=I,i=31,I=13,q=1", "");
        command(&mut term, "a=d,d=I,i=31,I=13,q=2", "");

        let einval = |keys: &str| format!("\x1b_Gi=31,I=13{keys};EINVAL:both i and I given\x1b\\");
        let expected = [einval(""), einval(""), einval(",p=2"), einval(""), einval("")];
        assert_eq!(replies.take(), expected);
        assert_eq!(stored(&term), [31]);
        assert!(term.kitty_virtual_placement(31, 0).is_some());
        assert!(term.kitty_virtual_placement(31, 2).is_none());
        assert_eq!(queues(&mut term), (Vec::new(), Vec::new()));
    }

    #[test]
    fn identical_virtual_placements_share_a_texture() {
        let (mut term, _) = term();
        command(&mut term, "a=t,f=100,i=9,q=2", &png(20, 20));
        command(&mut term, "a=p,U=1,i=9,p=1,c=2,r=1,q=2", "");
        command(&mut term, "a=p,U=1,i=9,p=2,c=2,r=1,q=2", "");
        // Replacing a placement with an identical one keeps the texture.
        command(&mut term, "a=p,U=1,i=9,p=1,c=2,r=1,q=2", "");
        let (uploaded, removed) = queues(&mut term);
        assert_eq!((uploaded.len(), removed.len()), (1, 0));
        let texture = uploaded[0].id;
        assert_eq!(term.kitty_virtual_placement(9, 2).unwrap().texture.id, texture);
        assert_eq!(term.graphics.kitty.placement_bytes, 20 * 20 * 4);

        // A different size has its own texture.
        command(&mut term, "a=p,U=1,i=9,p=3,c=1,r=1,q=2", "");
        assert_eq!(queues(&mut term).0.len(), 1);

        // The texture goes with its last placement.
        command(&mut term, "a=d,d=i,i=9,p=1", "");
        assert_eq!(queues(&mut term), (Vec::new(), Vec::new()));
        command(&mut term, "a=d,d=i,i=9,p=2", "");
        assert_eq!(queues(&mut term).1, [texture]);
        assert_eq!(term.graphics.kitty.placement_bytes, 10 * 20 * 4);
    }

    #[test]
    fn placement_quota_evicts_oldest_placements() {
        let (mut term, _) = term();
        // One red pixel; a one-cell placement is a 10x20 texture of 800 bytes.
        command(&mut term, "a=t,f=32,s=1,v=1,i=1,q=2", "/wAA/w==");
        term.graphics.kitty.placement_quota = 3 * 800;
        let place = |term: &mut Term<Replies>, control: &str| {
            command(term, &format!("a=p,U=1,i=1,q=2,{control}"), "");
        };

        place(&mut term, "p=1,c=1,r=1");
        place(&mut term, "p=2,c=2,r=1");
        // Shares the texture of placement 1.
        place(&mut term, "p=3,c=1,r=1");
        let (uploaded, _) = queues(&mut term);
        let (one, two) = (uploaded[0].id, uploaded[1].id);
        assert_eq!(uploaded.len(), 2);
        assert_eq!(term.graphics.kitty.placement_bytes, 3 * 800);

        // 2400 more bytes: placement 1 goes, freeing nothing, then 2, then 3 with the texture
        // of 1.
        fully_damaged(&mut term);
        place(&mut term, "p=4,c=3,r=1");
        let (uploaded, mut removed) = queues(&mut term);
        removed.sort_unstable();
        assert_eq!(removed, [one, two]);
        assert_eq!(uploaded.len(), 1);
        assert!((1..=3).all(|p| term.kitty_virtual_placement(1, p).is_none()));
        assert!(term.kitty_virtual_placement(1, 4).is_some());
        assert_eq!(term.graphics.kitty.placement_bytes, 3 * 800);
        assert!(fully_damaged(&mut term));

        // The placement limit counts direct placements too.
        term.graphics.kitty.placement_quota = usize::MAX;
        term.graphics.kitty.placement_limit = 2;
        command(&mut term, "a=p,i=1,p=5,C=1,q=2", "");
        place(&mut term, "p=6,c=1,r=1");
        assert!(term.kitty_virtual_placement(1, 4).is_none());
        assert!(term.kitty_virtual_placement(1, 6).is_some());
        assert_eq!(term.graphics.kitty.direct_placements.len(), 1);
        place(&mut term, "p=7,c=1,r=1");
        assert!(term.graphics.kitty.direct_placements.is_empty());
        assert_eq!(term.graphics.kitty.placement_order.len(), 2);

        // Whatever the sizes, the textures alive in the display stay within the quota.
        term.graphics.kitty.placement_quota = 10_000;
        term.graphics.kitty.placement_limit = PLACEMENT_LIMIT;
        queues(&mut term);
        let mut live = HashMap::new();
        for p in 8..40 {
            place(&mut term, &format!("p={p},c={},r={}", p % 4 + 1, p % 2 + 1));
            let (uploaded, removed) = queues(&mut term);
            live.extend(uploaded.iter().map(|graphic| (graphic.id, graphic.pixels.len())));
            for id in removed {
                live.remove(&id);
            }
            assert!(live.values().sum::<usize>() <= 10_000, "placement {p}");
        }
    }

    #[test]
    fn evicting_many_placed_images_is_fast() {
        const IMAGES: u32 = 10_000;

        let (mut term, _) = term();
        let kitty = &mut term.graphics.kitty;
        kitty.storage_quota = IMAGES as usize * 4;
        kitty.image_limit = usize::MAX;
        kitty.placement_limit = usize::MAX;
        for id in 1..=IMAGES {
            command(&mut term, &format!("a=t,f=32,s=1,v=1,i={id},q=2"), "/wAA/w==");
            command(&mut term, &format!("a=p,U=1,i={id},c=1,r=1,q=2"), "");
        }
        queues(&mut term);

        // Every image is placed, and the new one needs the room of all of them. Rescanning the
        // images and their placements for each eviction would take minutes.
        let start = Instant::now();
        let pixels = BASE64.encode(vec![0xff; IMAGES as usize * 4]);
        let control = format!("a=t,f=32,s=100,v={},i={},q=2", IMAGES / 100, IMAGES + 1);
        command(&mut term, &control, &pixels);
        assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());

        assert_eq!(stored(&term), [IMAGES + 1]);
        let kitty = &term.graphics.kitty;
        assert!(kitty.virtual_placements.is_empty() && kitty.placement_order.is_empty());
        assert_eq!((kitty.placement_bytes, kitty.stored_bytes), (0, IMAGES as usize * 4));
        assert_eq!(queues(&mut term).1.len(), IMAGES as usize);
    }
}
