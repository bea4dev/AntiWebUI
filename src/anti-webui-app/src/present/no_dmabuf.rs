//! Where there are no dmabufs: the presenter of our own buffers has no values, so that
//! `Presenter` reads the same on every OS and only ever has a GPU surface here.

use super::Stats;
use crate::host::Host;
use crate::pacing::PresentTrace;
use anti_webui_core::damage::Damage;
use anti_webui_core::platform::SurfaceId;
use anti_webui_core::scene::Scene;
use anti_webui_wgpu::{Gpu, GpuError};
use std::rc::Rc;

pub(crate) enum DmabufPresenter {}

pub(crate) enum DmabufTarget {}

impl DmabufTarget {
    pub fn drop_held(&mut self) {
        match *self {}
    }

    pub fn has_unsent(&self) -> bool {
        match *self {}
    }
}

impl DmabufPresenter {
    pub fn new(_ctx: &mut impl Host) -> Result<(Rc<Gpu>, Self), GpuError> {
        Err(GpuError::Dmabuf("there are no dmabufs on this OS".into()))
    }

    pub fn name(&self) -> &'static str {
        match *self {}
    }

    pub fn new_target(&self) -> DmabufTarget {
        match *self {}
    }

    pub fn can_hold_back(&self, _t: &DmabufTarget, _limit: usize) -> bool {
        match *self {}
    }

    pub fn commit_held(
        &mut self,
        _ctx: &mut impl Host,
        _surface: SurfaceId,
        _t: &mut DmabufTarget,
    ) -> Option<Option<u64>> {
        match *self {}
    }

    pub fn keep_picture_pixels(&mut self, _keep: bool) {
        match *self {}
    }

    pub fn video_render_node(&self) -> Option<std::path::PathBuf> {
        match *self {}
    }

    pub fn take_video_import_failed(&mut self) -> bool {
        match *self {}
    }

    pub fn forget(&mut self, _ctx: &mut impl Host, _target: u64, _buffers: Option<DmabufTarget>) {
        match *self {}
    }

    pub fn update(
        &mut self,
        _ctx: &mut impl Host,
        _targets: Vec<&mut DmabufTarget>,
    ) -> Result<(), GpuError> {
        match *self {}
    }

    pub fn release(&mut self, _ctx: &mut impl Host, _targets: Vec<&mut DmabufTarget>) {
        match *self {}
    }

    pub fn retire(&mut self, _targets: impl Iterator<Item = DmabufTarget>) {
        match *self {}
    }

    #[allow(clippy::too_many_arguments)]
    pub fn present(
        &mut self,
        _gpu: &Gpu,
        _ctx: &mut impl Host,
        _surface: SurfaceId,
        _target: &mut DmabufTarget,
        _scene: &Scene,
        _damage: &Damage,
        _trace: Option<&mut PresentTrace>,
    ) -> Result<Option<Stats>, GpuError> {
        match *self {}
    }
}
