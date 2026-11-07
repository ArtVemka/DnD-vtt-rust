// Tauri bridge: the renderer talks to the Rust backend through window.desktop,
// the same shape preload.js exposed under Electron.
(function () {
  'use strict';

  const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);

  async function call(command, args) {
    try {
      return await invoke(command, args);
    } catch (error) {
      return { ok: false, error: String((error && error.message) || error) };
    }
  }

  window.desktop = {
    isElectron: true,
    isAndroid: false,

    getConfig: () => call('app_config'),
    quitApp: () => call('app_quit'),
    setAutosaveInterval: (seconds) => call('app_autosave_interval', { seconds }),
    setPresence: () => Promise.resolve({ ok: true }),
    checkForUpdates: () => Promise.resolve({
      ok: false,
      error: 'Обновления появятся после настройки подписи сборки.'
    }),

    listCampaigns: () => call('campaign_list'),
    createCampaign: (name) => call('campaign_create', { name }),
    loadCampaign: (id) => call('campaign_load', { id }),
    currentCampaign: () => call('campaign_current'),
    saveCampaign: () => call('campaign_save'),
    backupCampaign: () => call('campaign_backup'),
    exportCampaign: () => call('campaign_export'),
    importCampaign: () => call('campaign_import'),
    backupInfo: () => call('campaign_backup_info'),
    restoreBackup: (id, name) => call('campaign_restore_backup', { id, backupName: name }),
    deleteCampaign: (id) => call('campaign_delete', { id }),

    startHost: () => call('host_start'),
    stopHost: () => call('host_stop'),
    hostStatus: () => call('host_status'),

    openDataDir: () => call('shell_open_data_dir'),
    openBackups: () => call('shell_open_backups'),
    openExternal: (url) => call('shell_open_external', { url }),

    // Event channels exist in Electron; the Rust backend has no producer yet.
    onCampaignSaved: () => () => {},
    onCampaignError: () => () => {},
    onHostLog: () => () => {}
  };
})();
