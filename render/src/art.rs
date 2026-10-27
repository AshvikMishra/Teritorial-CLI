//! ASCII art: the splash animation, title banner, countdown, victory and defeat
//! banners, and short flourishes for the event log.
//!
//! Everything here is plain printable ASCII, so it renders the same in every
//! terminal and in the ASCII renderer. Each piece is a block of lines padded to
//! the same width, at most 96 columns wide. Use [`size`] to center a piece.

/// Splash animation frame 1 of 4.
pub const SPLASH_0: &str = r#"........ ....... .......  .......  .... ........  ......  .......  ....  ......  ..      
   ..    ..      ..    .. ..    ..  ..     ..    ..    .. ..    ..  ..  ..    .. ..      
   ..    .....   .......  .......   ..     ..    ..    .. .......   ..  ........ ..      
   ..    ..      ..   ..  ..   ..   ..     ..    ..    .. ..   ..   ..  ..    .. ..      
   ..    ....... ..    .. ..    .. ....    ..     ......  ..    .. .... ..    .. ....... 
                                                                                         
                                                                                         
                                                                                         
                                                                                         
                                                                                         
                                                                                         
                                                                                         
                                                                                         
                                                                                         
                                                                                         "#;

/// Splash animation frame 2 of 4.
pub const SPLASH_1: &str = r#":::::::: ::::::: :::::::  :::::::  :::: ::::::::  ::::::  :::::::  ::::  ::::::  ::      
   ::    ::      ::    :: ::    ::  ::     ::    ::    :: ::    ::  ::  ::    :: ::      
   ::    :::::   :::::::  :::::::   ::     ::    ::    :: :::::::   ::  :::::::: ::      
   ::    ::      ::   ::  ::   ::   ::     ::    ::    :: ::   ::   ::  ::    :: ::      
   ::    ::::::: ::    :: ::    :: ::::    ::     ::::::  ::    :: :::: ::    :: ::::::: 
                                                                                         
                                                                                         
                                  ....... ..      ....                                   
                                 ..       ..       ..                                    
                                 ..       ..       ..                                    
                                 ..       ..       ..                                    
                                  ....... ....... ....                                   
                                                                                         
                                                                                         
                                                                                         "#;

/// Splash animation frame 3 of 4.
pub const SPLASH_2: &str = r#"######## ####### #######  #######  #### ########  ######  #######  ####  ######  ##      
   ##    ##      ##    ## ##    ##  ##     ##    ##    ## ##    ##  ##  ##    ## ##      
   ##    #####   #######  #######   ##     ##    ##    ## #######   ##  ######## ##      
   ##    ##      ##   ##  ##   ##   ##     ##    ##    ## ##   ##   ##  ##    ## ##      
   ##    ####### ##    ## ##    ## ####    ##     ######  ##    ## #### ##    ## ####### 
                                                                                         
                                                                                         
                                  ::::::: ::      ::::                                   
                                 ::       ::       ::                                    
                                 ::       ::       ::                                    
                                 ::       ::       ::                                    
                                  ::::::: ::::::: ::::                                   
                                                                                         
                                                                                         
                   ..................................................                    "#;

/// Splash animation frame 4 of 4.
pub const SPLASH_3: &str = r#"######## ####### #######  #######  #### ########  ######  #######  ####  ######  ##      
 ..##....##......##....## ##....##  ##.. ..##....##....## ##....##  ##..##....## ##.     
   ##.   #####   #######..#######.. ##.    ##.   ##.   ##.#######.. ##. ########.##.     
   ##.   ##....  ##...##. ##...##.  ##.    ##.   ##.   ##.##...##.  ##. ##....##.##.     
   ##.   ####### ##.   ## ##.   ## ####    ##.    ######..##.   ## #### ##.   ##.####### 
    ..    ....... ..    .. ..    .. ....    ..     ......  ..    .. .... ..    .. .......
                                                                                         
                                  ####### ##      ####                                   
                                 ##.......##.      ##..                                  
                                 ##.      ##.      ##.                                   
                                 ##.      ##.      ##.                                   
                                  ####### ####### ####                                   
                                   ....... ....... ....                                  
                                                                                         
                   -=[  conquer the world, one command at a time  ]=-                    "#;

/// The title banner: the TERRITORIAL logo with a CLI tag line.
pub const TITLE_BANNER: &str = r#"######## ####### #######  #######  #### ########  ######  #######  ####  ######  ##      
 ..##....##......##....## ##....##  ##.. ..##....##....## ##....##  ##..##....## ##.     
   ##.   #####   #######..#######.. ##.    ##.   ##.   ##.#######.. ##. ########.##.     
   ##.   ##....  ##...##. ##...##.  ##.    ##.   ##.   ##.##...##.  ##. ##....##.##.     
   ##.   ####### ##.   ## ##.   ## ####    ##.    ######..##.   ## #### ##.   ##.####### 
    ..    ....... ..    .. ..    .. ....    ..     ......  ..    .. .... ..    .. .......
                                      [ C L I ]  conquer the world, one command at a time"#;

/// Big countdown digit 3.
pub const COUNT_3: &str = r#"##########   
 .......#### 
        ####.
  ########...
   .....#### 
        ####.
##########...
 ..........  "#;

/// Big countdown digit 2.
pub const COUNT_2: &str = r#"  ########   
####....#### 
 ....   ####.
      ####...
    ####...  
  ####...    
############ 
 ............"#;

/// Big countdown digit 1.
pub const COUNT_1: &str = r#"    ####     
  ######.    
########.    
 ...####.    
    ####.    
    ####.    
############ 
 ............"#;

/// Big countdown "GO!".
pub const COUNT_GO: &str = r#"  ########      ########    #### 
####....####  ####....####  ####.
####.    .... ####.   ####. ####.
####. ######  ####.   ####. ####.
####.  .####. ####.   ####. ####.
####.   ####. ####.   ####.  ....
 .##########.  .########... #### 
   ..........    ........    ...."#;

/// Victory banner.
pub const VICTORY: &str = r#"  .    *        +        .         *        +        .     *        .
                                                                     
      ##    ## ####  ####### ########  ######  #######  ##    ##     
      ##.   ##. ##..##....... ..##....##....## ##....##  ##  ##..    
      ##.   ##. ##. ##.         ##.   ##.   ##.#######..  .## ..     
       ##  ##.. ##. ##.         ##.   ##.   ##.##...##.    ##.       
        .## .. ####  #######    ##.    ######..##.   ##    ##.       
          ..    ....  .......    ..     ......  ..    ..    ..       
                                                                     
           =========[  THE WORLD BOWS BEFORE YOU  ]=========         
                                                                     
    *        .        +        *        .        +        *        . "#;

/// Defeat banner.
pub const DEFEAT: &str = r#"    _  _  _  _  _  _  _  _  _  _  _  _  _  _  _  _  _  _  _  _  _  _
                                                                    
          #######  ####### ####### #######  ######  ########        
          ##....## ##......##......##......##....##  ..##....       
          ##.   ##.#####   #####   #####   ########.   ##.          
          ##.   ##.##....  ##....  ##....  ##....##.   ##.          
          #######..####### ##.     ####### ##.   ##.   ##.          
           .......  ....... ..      ....... ..    ..    ..          
                                                                    
             ---------[  YOUR NATION HAS FALLEN  ]---------         
                                                                    
             the banners are lowered, but the map remembers         "#;

/// The splash animation: the logo materializes over a few frames. All frames
/// have the same size. Show them in order and hold the last one.
pub const SPLASH_FRAMES: &[&str] = &[SPLASH_0, SPLASH_1, SPLASH_2, SPLASH_3];

/// The splash tag line (also part of the last splash frame).
pub const TAGLINE: &str = "conquer the world, one command at a time";

/// Splash frame `i`, holding the last frame for any `i` past the end.
pub fn splash_frame(i: usize) -> &'static str {
    let last = SPLASH_FRAMES.len() - 1;
    SPLASH_FRAMES[i.min(last)]
}

/// The big countdown banner for `n`: `3`, `2`, `1`, and `GO!` for `0`.
/// Values above 3 show `3`.
pub fn countdown(n: u32) -> &'static str {
    match n {
        0 => COUNT_GO,
        1 => COUNT_1,
        2 => COUNT_2,
        _ => COUNT_3,
    }
}

const CAPTURE_FLOURISHES: [&str; 8] = [
    "-=>", ">>>", "=)>", "~*~", "[+]", "<*>", "-+>", "=>>",
];

const ELIMINATION_FLOURISHES: [&str; 8] = [
    "[X]", "x_x", "-X-", "*X*", "}X{", "<X>", "#X#", "/X\\",
];

/// A short glyph string to decorate a capture in the event log. The same seed
/// always gives the same string, so a region id or tick is a good seed.
pub fn capture_flourish(seed: u32) -> &'static str {
    CAPTURE_FLOURISHES[(seed as usize) % CAPTURE_FLOURISHES.len()]
}

/// A short glyph string to decorate an elimination in the event log.
pub fn elimination_flourish(seed: u32) -> &'static str {
    ELIMINATION_FLOURISHES[(seed as usize) % ELIMINATION_FLOURISHES.len()]
}

/// The size of an art piece as `(columns, rows)`: the longest line in characters
/// and the number of lines.
pub fn size(art: &str) -> (u16, u16) {
    let mut w = 0usize;
    let mut h = 0usize;
    for line in art.lines() {
        w = w.max(line.chars().count());
        h += 1;
    }
    (w.min(u16::MAX as usize) as u16, h.min(u16::MAX as usize) as u16)
}
