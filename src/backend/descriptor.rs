use std::io::{Cursor, Read, Seek, SeekFrom};

use crate::HidResult;

#[derive(Default)]
pub struct HidrawReportDescriptor(Vec<u8>);

impl HidrawReportDescriptor {
    pub fn from_slice(value: &[u8]) -> HidResult<Self> {
        Ok(HidrawReportDescriptor(value.to_vec()))
    }

    pub fn usages(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        UsageIterator {
            usage_page: 0,
            depth: 0,
            cursor: Cursor::new(&self.0)
        }
    }
}

/// Iterates over the values in a HidrawReportDescriptor
struct UsageIterator<'a> {
    usage_page: u16,
    depth: u32,
    cursor: Cursor<&'a Vec<u8>>
}

impl Iterator for UsageIterator<'_> {
    type Item = (u16, u16);

    fn next(&mut self) -> Option<Self::Item> {
        next_hid_usage(&mut self.cursor, &mut self.usage_page, &mut self.depth)
    }
}

// This comes from hidapi which apparently comes from Apple's implementation of
// this
fn next_hid_usage(cursor: &mut Cursor<&Vec<u8>>, usage_page: &mut u16, depth: &mut u32) -> Option<(u16, u16)> {
    let mut usage = None;
    let mut usage_pair = None;
    let initial = cursor.position() == 0;

    while let Some(Ok(key)) = cursor.bytes().next() {
        // The amount to skip is calculated based off of the start of the
        // iteration so we need to keep track of that.
        let position = cursor.position() - 1;
        let key_cmd = key & 0xfc;

        let (data_len, key_size) = hid_item_size(key, cursor)?;

        match key_cmd {
            // Usage Page 6.2.2.7 (Global)
            0x4 => {
                *usage_page = match hid_report_bytes(cursor, data_len) {
                    Ok(v) => v as u16,
                    Err(_) => break,
                }
            }
            // Usage 6.2.2.8 (Local)
            0x8 => {
                // A 4-byte usage carries its own usage page in the high word.
                usage = match hid_report_bytes(cursor, data_len) {
                    Ok(v) => Some(((data_len == 4).then_some((v >> 16) as u16), v as u16)),
                    Err(_) => break,
                }
            }
            // Collection 6.2.2.4 (Main)
            // Only top-level collections describe a device, as in hidapi.
            0xa0 => {
                // Usage is a Local Item, unset it
                if let (0, Some((page, u))) = (*depth, usage.take()) {
                    usage_pair = Some((page.unwrap_or(*usage_page), u))
                }
                *depth += 1;
            }
            // Input 6.2.2.4 (Main)
            0x80 |
            // Output 6.2.2.4 (Main)
            0x90 |
            // Feature 6.2.2.4 (Main)
            0xb0 => {
                // Usage is a Local Item, unset it
                usage.take();
            }
            // End Collection 6.2.2.4 (Main)
            0xc0 => {
                usage.take();
                *depth = depth.saturating_sub(1);
            }
            _ => {}
        }

        if cursor
            .seek(SeekFrom::Start(position + (data_len + key_size) as u64))
            .is_err()
        {
            return None;
        }

        if let Some((usage_page, usage)) = usage_pair {
            return Some((usage_page, usage));
        }
    }

    if let (true, Some((page, usage))) = (initial, usage) {
        return Some((page.unwrap_or(*usage_page), usage));
    }

    None
}

/// Gets the size of the HID item at the given position
///
/// Returns data_len and key_size when successful
fn hid_item_size(key: u8, cursor: &mut Cursor<&Vec<u8>>) -> Option<(usize, usize)> {
    // Long Item. Next byte contains the length of the data section.
    if (key & 0xf0) == 0xf0 {
        if let Some(Ok(len)) = cursor.bytes().next() {
            return Some((len.into(), 3));
        }

        // Malformed report
        return None;
    }

    // Short Item. Bottom two bits contains the size code
    match key & 0x03 {
        v @ 0..=2 => Some((v.into(), 1)),
        3 => Some((4, 1)),
        _ => unreachable!() // & 0x03 means this can't happen
    }
}

/// Get the bytes from a HID report descriptor
///
/// Must only be called with `num_bytes` 0, 1, 2 or 4.
fn hid_report_bytes(cursor: &mut Cursor<&Vec<u8>>, num_bytes: usize) -> HidResult<u32> {
    let mut bytes: [u8; 4] = [0; 4];
    cursor.read_exact(&mut bytes[..num_bytes])?;

    Ok(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::HidrawReportDescriptor;

    fn usages(descriptor: &[u8]) -> Vec<(u16, u16)> {
        HidrawReportDescriptor::from_slice(descriptor)
            .unwrap()
            .usages()
            .collect()
    }

    #[test]
    fn nested_collections_are_not_listed() {
        // Mouse: Application collection with a Physical collection for the pointer.
        let mouse = [0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x09, 0x01, 0xa1, 0x00, 0xc0, 0xc0];
        assert_eq!(usages(&mouse), [(0x01, 0x02)]);
    }

    #[test]
    fn top_level_collections_are_listed_in_order() {
        let two = [0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0xc0, 0x09, 0x06, 0xa1, 0x01, 0xc0];
        assert_eq!(usages(&two), [(0x01, 0x02), (0x01, 0x06)]);
    }

    #[test]
    fn a_four_byte_usage_carries_its_usage_page() {
        let extended = [0x0b, 0x02, 0x00, 0x01, 0x00, 0xa1, 0x01, 0xc0];
        assert_eq!(usages(&extended), [(0x01, 0x02)]);
        // It does not change the usage page of the following usages.
        let mixed = [0x05, 0x01, 0x0b, 0x02, 0x00, 0x0c, 0x00, 0xa1, 0x01, 0xc0, 0x09, 0x06, 0xa1, 0x01, 0xc0];
        assert_eq!(usages(&mixed), [(0x0c, 0x02), (0x01, 0x06)]);
    }
}
