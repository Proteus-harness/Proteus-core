use std::io::{BufReader, Cursor};

use super::*;

// Frozen pre-change implementation: compare values, diagnostics and consumed
// bytes, not merely whether the optimized reader accepts valid JSON.
fn read_reference<R: BufRead>(reader: &mut R, max_frame_bytes: usize) -> Result<Value> {
    let mut line = Vec::with_capacity(max_frame_bytes.min(8192));
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            if line.is_empty() {
                bail!("child stdout closed before a frame was received");
            }
            break;
        }
        let bytes_to_take = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |position| position + 1);
        if line.len().saturating_add(bytes_to_take) > max_frame_bytes {
            bail!("newline JSON frame exceeded {max_frame_bytes} bytes before newline");
        }
        line.extend_from_slice(&buffer[..bytes_to_take]);
        reader.consume(bytes_to_take);
        if line.last() == Some(&b'\n') {
            break;
        }
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let line = std::str::from_utf8(&line)?;
    serde_json::from_str(line).map_err(Into::into)
}

fn consumed(reader: &BufReader<Cursor<&[u8]>>) -> usize {
    reader.get_ref().position() as usize - reader.buffer().len()
}

#[test]
fn complete_buffered_frames_preserve_values_and_next_frame_boundary() {
    let input = "{\"text\":\"Привет 🦀\\n\"}\r\n[null,true,false]\n{}\n";
    let mut reader = Cursor::new(input.as_bytes());
    let framing = NewlineJsonFraming::new(1024);
    let first_boundary = input.find('\n').unwrap() + 1;
    assert_eq!(
        framing.read_frame(&mut reader).unwrap(),
        serde_json::json!({"text": "Привет 🦀\n"})
    );
    assert_eq!(reader.position() as usize, first_boundary);
    assert_eq!(
        framing.read_frame(&mut reader).unwrap(),
        serde_json::json!([null, true, false])
    );
    assert_eq!(
        framing.read_frame(&mut reader).unwrap(),
        serde_json::json!({})
    );
    assert_eq!(reader.position() as usize, input.len());
}

#[test]
fn all_chunk_boundaries_and_limits_match_the_old_reader() {
    let cases: &[&[u8]] = &[
        b"",
        b"\n",
        b"\r\n",
        b"{}",
        b"{}\r",
        b"{}\n",
        b"{}\r\n",
        b"{}\r\r\n",
        b"{}\n[]\n",
        b"   {}  \ntrailing",
        b"[null,true,false,-0.5,12]\n",
        b"\"escaped \\\" \\n \\u0000\"\n",
        b"{bad}\n{}\n",
        b"{}x\n",
        b"\xff\n{}\n",
        b"\"\xc3\"\n{}\n",
        "{\"🦀\":\"\u{2028}\u{2029}\"}\r\n".as_bytes(),
    ];
    for &input in cases {
        for capacity in 1..=input.len().max(1) + 1 {
            for limit in 0..=input.len() + 1 {
                let mut reference = BufReader::with_capacity(capacity, Cursor::new(input));
                let mut candidate = BufReader::with_capacity(capacity, Cursor::new(input));
                let expected =
                    read_reference(&mut reference, limit).map_err(|error| error.to_string());
                let actual = NewlineJsonFraming::new(limit)
                    .read_frame(&mut candidate)
                    .map_err(|error| error.to_string());
                assert_eq!(
                    actual, expected,
                    "input {input:?}, chunk {capacity}, limit {limit}"
                );
                assert_eq!(
                    consumed(&candidate),
                    consumed(&reference),
                    "consumption: input {input:?}, chunk {capacity}, limit {limit}"
                );
            }
        }
    }
}

#[test]
fn invalid_complete_frame_is_consumed_before_error_but_not_the_next_frame() {
    for input in [b"\xff\n{}\n".as_slice(), b"{bad}\n{}\n".as_slice()] {
        for capacity in [1, input.len()] {
            let mut reader = BufReader::with_capacity(capacity, Cursor::new(input));
            let framing = NewlineJsonFraming::new(1024);
            assert!(framing.read_frame(&mut reader).is_err());
            assert_eq!(
                consumed(&reader),
                input.iter().position(|&byte| byte == b'\n').unwrap() + 1
            );
            assert_eq!(
                framing.read_frame(&mut reader).unwrap(),
                serde_json::json!({})
            );
            assert_eq!(consumed(&reader), input.len());
        }
    }
}

#[test]
fn limit_includes_delimiter_and_checks_only_the_current_frame() {
    let mut reader = Cursor::new(b"{}\n[]\n".as_slice());
    assert!(NewlineJsonFraming::new(2).read_frame(&mut reader).is_err());
    assert_eq!(reader.position(), 0, "oversize must fail before consume");
    let framing = NewlineJsonFraming::new(3);
    assert_eq!(
        framing.read_frame(&mut reader).unwrap(),
        serde_json::json!({})
    );
    assert_eq!(reader.position(), 3);
    assert_eq!(
        framing.read_frame(&mut reader).unwrap(),
        serde_json::json!([])
    );
    assert_eq!(reader.position(), 6);
}
