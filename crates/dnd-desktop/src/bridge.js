// Tauri bridge: the renderer talks to the Rust backend through window.desktop,
// the same shape preload.js exposed under Electron.
(function () {
  'use strict';

  const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);

  function listen(name, callback) {
    const pending = window.__TAURI__.event.listen(name, (event) => callback(event.payload));
    return () => {
      pending.then((unlisten) => unlisten()).catch(() => {});
    };
  }

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
    setPresence: (state) => call('presence_update', { input: state }),
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

    irohStart: () => call('iroh_start'),
    irohStop: () => call('iroh_stop'),
    irohCreateTicket: () => call('iroh_create_ticket'),
    irohTicketQr: (ticketStr) => call('iroh_ticket_qr', { ticketStr }),
    irohTunnelClient: (ticketStr) => call('iroh_tunnel_client', { ticketStr }),
    irohTunnelStop: () => call('iroh_tunnel_stop'),

    openDataDir: () => call('shell_open_data_dir'),
    openBackups: () => call('shell_open_backups'),
    openExternal: (url) => call('shell_open_external', { url }),

    openSheetWindow: (request) => call('sheet_open_window', { request }),
    openJournalWindow: (request) => call('journal_open_window', { request }),

    pickAdventureFile: () => call('adventure_pick_file'),
    inspectAdventure: (filePath) => call('adventure_inspect', { filePath }),
    importAdventure: (filePath, include) => call('adventure_import', { filePath, include }),

    onCampaignSaved: (callback) => listen('campaign-saved', callback),
    onCampaignError: (callback) => listen('campaign-error', callback),
    onHostLog: (callback) => listen('host-log', callback)
  };
})();
