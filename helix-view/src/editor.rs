use crate::{theme::Theme, tree::Tree, Document, DocumentId, View, ViewId};
use tui::layout::Rect;

use std::path::PathBuf;

use slotmap::SlotMap;

use anyhow::Error;

pub struct Editor {
    pub tree: Tree,
    pub documents: SlotMap<DocumentId, Document>,
    pub count: Option<usize>,
    pub theme: Theme,
    pub language_servers: helix_lsp::Registry,
    pub executor: &'static smol::Executor<'static>,
}

#[derive(Copy, Clone)]
pub enum Action {
    Replace,
    HorizontalSplit,
    VerticalSplit,
}

impl Editor {
    pub fn new(executor: &'static smol::Executor<'static>, mut area: tui::layout::Rect) -> Self {
        // TODO: load from config dir
        let toml = include_str!("../../theme.toml");
        let theme: Theme = toml::from_str(&toml).expect("failed to parse theme.toml");

        let language_servers = helix_lsp::Registry::new();

        // HAXX: offset the render area height by 1 to account for prompt/commandline
        area.height -= 1;

        Self {
            tree: Tree::new(area),
            documents: SlotMap::with_key(),
            count: None,
            theme,
            language_servers,
            executor,
        }
    }

    fn _refresh(&mut self) {
        for (view, _) in self.tree.views_mut() {
            let doc = &self.documents[view.doc];
            view.ensure_cursor_in_view(doc)
        }
    }

    pub fn open(&mut self, path: PathBuf, action: Action) -> Result<DocumentId, Error> {
        let id = self
            .documents()
            .find(|doc| doc.path() == Some(&path))
            .map(|doc| doc.id);

        let id = if let Some(id) = id {
            id
        } else {
            let mut doc = Document::load(path, self.theme.scopes())?;

            // try to find a language server based on the language name
            let language_server = doc
                .language
                .as_ref()
                .and_then(|language| self.language_servers.get(language, self.executor));

            if let Some(language_server) = language_server {
                doc.set_language_server(Some(language_server.clone()));

                let language_id = doc
                    .language()
                    .and_then(|s| s.split('.').last()) // source.rust
                    .map(ToOwned::to_owned)
                    .unwrap_or_default();

                smol::block_on(language_server.text_document_did_open(
                    doc.url().unwrap(),
                    doc.version(),
                    doc.text(),
                    language_id,
                ))
                .unwrap();
            }

            let id = self.documents.insert(doc);
            self.documents[id].id = id;
            id
        };

        use crate::tree::Layout;
        use helix_core::Selection;
        match action {
            Action::Replace => {
                let view = self.view();
                let jump = (
                    view.doc,
                    self.documents[view.doc].selection(view.id).clone(),
                );

                let view = self.view_mut();
                view.jumps.push(jump);
                view.doc = id;
                view.first_line = 0;
                let view_id = view.id;

                // initialize selection for view
                let doc = &mut self.documents[id];
                doc.selections.insert(view_id, Selection::point(0));

                return Ok(id);
            }
            Action::HorizontalSplit => {
                let view = View::new(id)?;
                let view_id = self.tree.split(view, Layout::Horizontal);
                // initialize selection for view
                let doc = &mut self.documents[id];
                doc.selections.insert(view_id, Selection::point(0));
            }
            Action::VerticalSplit => {
                let view = View::new(id)?;
                let view_id = self.tree.split(view, Layout::Vertical);
                // initialize selection for view
                let doc = &mut self.documents[id];
                doc.selections.insert(view_id, Selection::point(0));
            }
        }

        self._refresh();

        Ok(id)
    }

    pub fn close(&mut self, id: ViewId) {
        let view = self.tree.get(self.tree.focus);
        // get around borrowck issues
        let language_servers = &mut self.language_servers;
        let executor = self.executor;

        let doc = &self.documents[view.doc];

        let language_server = doc
            .language
            .as_ref()
            .and_then(|language| language_servers.get(language, executor));

        if let Some(language_server) = language_server {
            smol::block_on(language_server.text_document_did_close(doc.identifier())).unwrap();
        }

        // remove selection
        self.documents[view.doc].selections.remove(&id);

        // self.documents.remove(view.doc);
        self.tree.remove(id);
        self._refresh();
    }

    pub fn resize(&mut self, area: Rect) {
        self.tree.resize(area);
        self._refresh();
    }

    pub fn focus_next(&mut self) {
        self.tree.focus_next();
    }

    pub fn should_close(&self) -> bool {
        self.tree.is_empty()
    }

    pub fn current(&mut self) -> (&mut View, &mut Document) {
        let view = self.tree.get_mut(self.tree.focus);
        let doc = &mut self.documents[view.doc];
        (view, doc)
    }

    pub fn view(&self) -> &View {
        self.tree.get(self.tree.focus)
    }

    pub fn view_mut(&mut self) -> &mut View {
        self.tree.get_mut(self.tree.focus)
    }

    pub fn ensure_cursor_in_view(&mut self, id: ViewId) {
        let view = self.tree.get_mut(id);
        let doc = &self.documents[view.doc];
        view.ensure_cursor_in_view(doc)
    }

    pub fn document(&self, id: DocumentId) -> Option<&Document> {
        self.documents.get(id)
    }

    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.documents.iter().map(|(_id, doc)| doc)
    }

    // pub fn current_document(&self) -> Document {
    //     let id = self.view().doc;
    //     let doc = &mut editor.documents[id];
    // }

    pub fn cursor_position(&self) -> Option<helix_core::Position> {
        const OFFSET: u16 = 7; // 1 diagnostic + 5 linenr + 1 gutter
        let view = self.view();
        let doc = &self.documents[view.doc];
        let cursor = doc.selection(view.id).cursor();
        if let Some(mut pos) = view.screen_coords_at_pos(doc, doc.text().slice(..), cursor) {
            pos.col += view.area.x as usize + OFFSET as usize;
            pos.row += view.area.y as usize;
            return Some(pos);
        }
        None
    }
}
