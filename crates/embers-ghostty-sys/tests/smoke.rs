//! The library links and its calls match the bindings: a terminal takes
//! output and reports the cell and cursor it leaves.

use std::ptr;

use embers_ghostty_sys::*;

#[test]
fn a_terminal_shows_what_is_written_to_it() {
    unsafe {
        let mut terminal: GhosttyTerminal = ptr::null_mut();
        let result = ghostty_terminal_new(ptr::null(), &mut terminal, 10, 2);
        assert_eq!(result, GhosttyResult::GHOSTTY_SUCCESS);

        let output = "漢x".as_bytes();
        ghostty_terminal_vt_write(terminal, output.as_ptr(), output.len());

        let mut cursor_x: u16 = 0;
        let result = ghostty_terminal_get(
            terminal,
            GhosttyTerminalData::GHOSTTY_TERMINAL_DATA_CURSOR_X,
            (&raw mut cursor_x).cast(),
        );
        assert_eq!(result, GhosttyResult::GHOSTTY_SUCCESS);
        assert_eq!(cursor_x, 3);

        let point = GhosttyPoint {
            tag: GhosttyPointTag::GHOSTTY_POINT_TAG_ACTIVE,
            value: GhosttyPointValue {
                coordinate: GhosttyPointCoordinate { x: 2, y: 0 },
            },
        };
        let mut cell = GhosttyGridRef {
            size: size_of::<GhosttyGridRef>(),
            ..Default::default()
        };
        let result = ghostty_terminal_grid_ref(terminal, point, &mut cell);
        assert_eq!(result, GhosttyResult::GHOSTTY_SUCCESS);
        let mut codepoints = [0_u32; 4];
        let mut len = 0;
        let result =
            ghostty_grid_ref_graphemes(&cell, codepoints.as_mut_ptr(), codepoints.len(), &mut len);
        assert_eq!(result, GhosttyResult::GHOSTTY_SUCCESS);
        assert_eq!(&codepoints[..len], &[u32::from('x')]);

        ghostty_terminal_free(terminal);
    }
}
