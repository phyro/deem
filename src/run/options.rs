/// What a run takes besides its program and input: how records are read
/// and printed, the named values, and the run's limits and records.
#[derive(Debug, Default, PartialEq)]
pub struct RunOptions {
    /// `--arg NAME VALUE`, and `-t N` as `threshold`
    pub named: Vec<(String, String)>,
    /// `--rawfile NAME FILE`: `$NAME` is the file's text
    pub rawfile: Vec<(String, String)>,
    /// `--img NAME FILE`: `$NAME` is the picture
    pub img: Vec<(String, String)>,
    pub json_in: bool,
    /// `-I`: each stdin line is a picture's path
    pub paths_in: bool,
    pub slurp: bool,
    pub split: Option<String>,
    pub raw_out: bool,
    pub exit_status: bool,
    pub stats: bool,
    pub trace: Option<String>,
    pub failed: Option<String>,
    pub max_questions: Option<u64>,
}
