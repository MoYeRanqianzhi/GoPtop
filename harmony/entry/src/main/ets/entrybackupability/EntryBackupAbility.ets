import { hilog } from '@kit.PerformanceAnalysisKit';
import { BackupExtensionAbility, BundleVersion } from '@kit.CoreFileKit';

const DOMAIN = 0x0000;

export default class EntryBackupAbility extends BackupExtensionAbility {
  /**
   * 空实现 = 不自定义备份集，交给系统默认规则：backup_config.json 为 allowToBackupRestore: true
   * 且未配 includes，默认集含 el2 的 haps/<module>/files/，即 Index.ets 落盘的 store.json 也在
   * 备份范围内（重装后可能被恢复）。要改范围改 backup_config.json 的 includes/excludes。
   */
  async onBackup() {
    hilog.info(DOMAIN, 'testTag', 'onBackup ok');
    await Promise.resolve();
  }

  async onRestore(bundleVersion: BundleVersion) {
    hilog.info(DOMAIN, 'testTag', 'onRestore ok %{public}s', JSON.stringify(bundleVersion));
    await Promise.resolve();
  }
}