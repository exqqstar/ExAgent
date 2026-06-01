use exagent::app_server::desktop_facade::DesktopFacade;
use exagent::index_db::IndexDb;
use tokio::sync::RwLock;

use crate::settings::DesktopSettingsStore;

pub struct DesktopState {
    pub facade: RwLock<DesktopFacade>,
    pub index: IndexDb,
    pub settings: DesktopSettingsStore,
}

impl DesktopState {
    pub fn new(facade: DesktopFacade, index: IndexDb, settings: DesktopSettingsStore) -> Self {
        Self {
            facade: RwLock::new(facade),
            index,
            settings,
        }
    }
}
