pub fn get_commands_to_draw_rect(width: usize, height: usize, color: u32) -> String {
    let mut draw_commands = String::new();

    for x in 0..width {
        for y in 0..height {
            draw_commands += &format!("PX {x} {y} {color:06x}\n");
        }
    }

    draw_commands
}

pub fn get_commands_to_read_rect(width: usize, height: usize) -> String {
    let mut read_commands = String::new();

    for x in 0..width {
        for y in 0..height {
            read_commands += &format!("PX {x} {y}\n");
        }
    }

    read_commands
}
