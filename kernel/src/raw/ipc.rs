//! Channels as kernel objects, in a table of slots. A handle to a channel
//! end stands for its [`Object`] number, and a channel goes once both its
//! ends are closed. An object a message carries is closed along with the
//! end the message waits for, should nobody take it.

use alloc::boxed::Box;
use alloc::vec::Vec;

use channel::{Attachment, Channel, End, Error};
use kernel::object::Object;

use super::memory::Spin;

/// How many channels there can be at once.
const CHANNELS: usize = 256;

static ALL: Spin<[Option<Box<Channel>>; CHANNELS]> = Spin::new([const { None }; CHANNELS]);

/// A new channel: the object numbers of its two ends, or `None` when every
/// slot is taken.
pub fn create() -> Option<(u64, u64)> {
    let mut all = ALL.lock();
    let slot = all.iter().position(Option::is_none)?;
    all[slot] = Some(Box::new(Channel::new()));
    let end = |end| Object::ChannelEnd { slot, end }.number();
    Some((end(End::A), end(End::B)))
}

/// Runs `with` on the channel and end `object` stands for; a number that
/// is no channel end, or one whose channel is gone, is a closed channel.
fn on_end<R>(
    object: u64,
    with: impl FnOnce(&mut Channel, End) -> Result<R, Error>,
) -> Result<R, Error> {
    let Some(Object::ChannelEnd { slot, end }) = Object::from_number(object) else {
        return Err(Error::Closed);
    };
    let mut all = ALL.lock();
    let channel = all
        .get_mut(slot)
        .and_then(Option::as_mut)
        .ok_or(Error::Closed)?;
    with(channel, end)
}

/// Sends `message` through the end `object` stands for, carrying
/// `attached` along.
///
/// # Errors
///
/// Those of [`Channel::send`]; the caller keeps `attached` then.
pub fn send(object: u64, message: &[u8], attached: Option<Attachment>) -> Result<(), Error> {
    on_end(object, |c, end| c.send_with(end, message, attached))
}

/// Takes the next message for the end `object` stands for into `into`,
/// with what it carried along.
///
/// # Errors
///
/// Those of [`Channel::receive`].
pub fn receive(object: u64, into: &mut [u8]) -> Result<(usize, Option<Attachment>), Error> {
    on_end(object, |c, end| c.receive(end, into))
}

/// Closes the end `object` stands for; once both ends are closed, the
/// channel goes. The objects of messages that waited for it are closed
/// too, and theirs in turn.
pub fn close(object: u64) {
    let mut closing = Vec::from([object]);
    while let Some(object) = closing.pop() {
        let Some(Object::ChannelEnd { slot, end }) = Object::from_number(object) else {
            continue;
        };
        let mut all = ALL.lock();
        let Some(entry) = all.get_mut(slot) else {
            continue;
        };
        if let Some(channel) = entry.as_mut() {
            channel.close(end, |dropped| closing.push(dropped.object));
            if channel.finished() {
                *entry = None;
            }
        }
    }
}

/// How many channels there are. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn live() -> usize {
    ALL.lock().iter().filter(|c| c.is_some()).count()
}
