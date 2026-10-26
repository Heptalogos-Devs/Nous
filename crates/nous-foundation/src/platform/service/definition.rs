use nous_domain::{Error, Result};
use xml::writer::{EmitterConfig, EventWriter, XmlEvent};

pub(super) struct Document {
    writer: EventWriter<Vec<u8>>,
}
impl Document {
    pub fn new() -> Self {
        Self {
            writer: EmitterConfig::new()
                .perform_indent(true)
                .write_document_declaration(false)
                .create_writer(Vec::new()),
        }
    }
    pub fn start(&mut self, name: &str, attributes: &[(&str, &str)]) -> Result<()> {
        let mut element = XmlEvent::start_element(name);
        for (key, value) in attributes {
            element = element.attr(*key, value);
        }
        self.writer.write(element).map_err(Error::dependency)
    }
    pub fn end(&mut self) -> Result<()> {
        self.writer
            .write(XmlEvent::end_element())
            .map_err(Error::dependency)
    }
    pub fn text(&mut self, name: &str, text: &str) -> Result<()> {
        self.start(name, &[])?;
        self.writer
            .write(XmlEvent::characters(text))
            .map_err(Error::dependency)?;
        self.end()
    }
    pub fn finish(self) -> Result<String> {
        String::from_utf8(self.writer.into_inner()).map_err(Error::dependency)
    }
}
#[cfg(windows)]
pub(super) fn windows_argument(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            '\\',
            if c == '"' { slashes * 2 + 1 } else { slashes },
        ));
        result.push(c);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}
