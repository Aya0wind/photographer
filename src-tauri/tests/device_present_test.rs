//! 启动存量设备枚举（devices::present）测试：
//! 注册过滤决策矩阵（纯函数：网络盘排除/无媒体可移动盘跳过/本地硬盘排除）、
//! 盘符掩码解码复用热插语义、真机卷枚举烟测（不触 WPD，真机 MTP 留手动验收）。

mod common;

pub use common::{db, devices, events, import, ipc, metadata, settings, tasks, thumbs};

use devices::hotplug::unitmask_to_drives;
use devices::present::{
    enumerate_present_volumes, is_registrable_volume, DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR,
    DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE, DRIVE_UNKNOWN,
};

#[test]
fn registrable_volume_matrix() {
    // 有媒体的可移动卷（读卡器/U 盘）→ 注册
    assert!(is_registrable_volume(DRIVE_REMOVABLE, true));
    // 无媒体的可移动盘（空卡槽）→ 跳过
    assert!(!is_registrable_volume(DRIVE_REMOVABLE, false));
    // 有媒体的光盘（照片 CD）属可移动介质 → 注册；无媒体跳过
    assert!(is_registrable_volume(DRIVE_CDROM, true));
    assert!(!is_registrable_volume(DRIVE_CDROM, false));
    // 本地硬盘（DRIVE_FIXED）不当导入设备
    assert!(!is_registrable_volume(DRIVE_FIXED, true));
    assert!(!is_registrable_volume(DRIVE_FIXED, false));
    // 网络盘（DRIVE_REMOTE，如 Y:/Z: 映射盘）不是导入源，绝不注册
    //（映射盘在会话/网络恢复时也会触发卷到达事件——热插与启动共用本过滤）
    assert!(!is_registrable_volume(DRIVE_REMOTE, true));
    assert!(!is_registrable_volume(DRIVE_REMOTE, false));
    // RAMDISK / 未知 / 无根目录一律排除
    assert!(!is_registrable_volume(DRIVE_RAMDISK, true));
    assert!(!is_registrable_volume(DRIVE_UNKNOWN, true));
    assert!(!is_registrable_volume(DRIVE_NO_ROOT_DIR, true));
    // 未知返回值防御（未来新增类型默认不注册）
    assert!(!is_registrable_volume(999, true));
}

#[test]
fn present_volume_enumeration_smoke_on_real_machine() {
    // 真机烟测：不 panic、条目为盘符形态、本地系统盘绝不出现
    //（C: 固定盘被过滤；测试机若插着有媒体的可移动盘会出现——只做性质断言）
    let volumes = enumerate_present_volumes();
    for (drive, label) in &volumes {
        assert_eq!(drive.len(), 2, "盘符形态 X: : {drive}");
        assert!(drive.ends_with(':'));
        assert!(!label.is_empty(), "注册时必须带回退卷标");
    }
    assert!(
        !volumes.iter().any(|(d, _)| d == "C:"),
        "本地系统盘不得注册为设备: {volumes:?}"
    );
}

#[test]
fn bitmask_decode_reuses_hotplug_semantics() {
    // GetLogicalDrives 掩码与 DBT dbcv_unitmask 同格式（bit0='A'）——直接复用热插解码
    assert_eq!(unitmask_to_drives(0b101), ["A:", "C:"]);
    assert_eq!(unitmask_to_drives(1 << 25), ["Z:"]);
    assert!(unitmask_to_drives(0).is_empty());
}
