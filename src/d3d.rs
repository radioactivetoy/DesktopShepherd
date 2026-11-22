//! Draws egui's output (triangles + textures) with Direct3D 11 into a transparent swap chain
//! that Windows' composition system shows. One renderer (one GPU device) serves every box;
//! each box has its own swap chain. Colours are egui's premultiplied sRGB values, blended as
//! premultiplied alpha, so a box's contents look the same as with the previous renderer.

#![cfg(windows)]

use egui::{self, epaint::Primitive, ClippedPrimitive, TextureId, TexturesDelta};
use std::collections::HashMap;
use windows::core::{s, Interface, Result, PCSTR};
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

const SHADER: &str = r#"
cbuffer C : register(b0) { float2 screen; float2 pad; };
struct VSIn { float2 pos : POSITION; float2 uv : TEXCOORD; float4 col : COLOR; };
struct PSIn { float4 pos : SV_POSITION; float2 uv : TEXCOORD; float4 col : COLOR; };
PSIn vs(VSIn i) {
    PSIn o;
    o.pos = float4(2.0 * i.pos.x / screen.x - 1.0, 1.0 - 2.0 * i.pos.y / screen.y, 0.0, 1.0);
    o.uv = i.uv;
    o.col = i.col;
    return o;
}
Texture2D t : register(t0);
SamplerState smp : register(s0);
float4 ps(PSIn i) : SV_TARGET { return i.col * t.Sample(smp, i.uv); }
"#;

struct Tex {
    tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    size: [usize; 2],
}

pub struct Renderer {
    pub device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    factory: IDXGIFactory2,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    layout: ID3D11InputLayout,
    cbuf: ID3D11Buffer,
    blend: ID3D11BlendState,
    raster: ID3D11RasterizerState,
    sampler: ID3D11SamplerState,
    textures: HashMap<TextureId, Tex>,
    vbuf: Option<(ID3D11Buffer, usize)>,
    ibuf: Option<(ID3D11Buffer, usize)>,
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<Vec<u8>> {
    unsafe {
        let mut code: Option<ID3DBlob> = None;
        let mut err: Option<ID3DBlob> = None;
        D3DCompile(SHADER.as_ptr() as *const _, SHADER.len(), s!("egui"), None, None, entry, target, 0, 0, &mut code, Some(&mut err))?;
        let blob = code.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        Ok(std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()).to_vec())
    }
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            // The graphics card first; if it can't (old GPU, some Remote Desktop setups),
            // Windows' software DirectX ("WARP"): same pictures, drawn by the processor.
            let mut made = Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
            for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
                made = D3D11CreateDevice(
                    None,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut ctx),
                );
                if made.is_ok() {
                    if driver == D3D_DRIVER_TYPE_WARP {
                        crate::storage::log_line("graphics card unavailable for DirectX; using software DirectX (WARP)");
                    }
                    break;
                }
            }
            made?;
            let fail = || windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL);
            let device = device.ok_or_else(fail)?;
            let ctx = ctx.ok_or_else(fail)?;
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;

            let vs_code = compile(s!("vs"), s!("vs_4_0"))?;
            let ps_code = compile(s!("ps"), s!("ps_4_0"))?;
            let mut vs = None;
            device.CreateVertexShader(&vs_code, None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(&ps_code, None, Some(&mut ps))?;
            let elems = [
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("POSITION"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 0,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("TEXCOORD"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 8,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: s!("COLOR"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    InputSlot: 0,
                    AlignedByteOffset: 16,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
            ];
            let mut layout = None;
            device.CreateInputLayout(&elems, &vs_code, Some(&mut layout))?;
            let mut cbuf = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 16,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut cbuf),
            )?;
            // Premultiplied alpha: result = src + (1 - src.a) * dst.
            let mut bd = D3D11_BLEND_DESC::default();
            bd.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut blend = None;
            device.CreateBlendState(&bd, Some(&mut blend))?;
            let mut raster = None;
            device.CreateRasterizerState(
                &D3D11_RASTERIZER_DESC {
                    FillMode: D3D11_FILL_SOLID,
                    CullMode: D3D11_CULL_NONE,
                    ScissorEnable: true.into(),
                    DepthClipEnable: false.into(),
                    ..Default::default()
                },
                Some(&mut raster),
            )?;
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            Ok(Self {
                device,
                ctx,
                factory,
                vs: vs.ok_or_else(fail)?,
                ps: ps.ok_or_else(fail)?,
                layout: layout.ok_or_else(fail)?,
                cbuf: cbuf.ok_or_else(fail)?,
                blend: blend.ok_or_else(fail)?,
                raster: raster.ok_or_else(fail)?,
                sampler: sampler.ok_or_else(fail)?,
                textures: HashMap::new(),
                vbuf: None,
                ibuf: None,
            })
        }
    }

    /// A transparent swap chain for one box (shown by Windows' composition system).
    pub fn create_swapchain(&self, w: u32, h: u32) -> Result<IDXGISwapChain1> {
        unsafe {
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w.max(1),
                Height: h.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            self.factory.CreateSwapChainForComposition(&self.device, &desc, None)
        }
    }

    /// An ordinary (not see-through) swap chain for a normal window: menus and settings.
    pub fn create_swapchain_for_hwnd(&self, hwnd: windows::Win32::Foundation::HWND, w: u32, h: u32) -> Result<IDXGISwapChain1> {
        unsafe {
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w.max(1),
                Height: h.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                // While the person drags the window bigger, new areas stay empty instead of
                // showing a stretched picture; the next frame fills them.
                Scaling: DXGI_SCALING_NONE,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let sc = self.factory.CreateSwapChainForHwnd(&self.device, hwnd, &desc, None, None)?;
            // No Alt+Enter full-screen switching.
            let _ = self.factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            Ok(sc)
        }
    }

    pub fn update_textures(&mut self, delta: &TexturesDelta) {
        for (id, d) in &delta.set {
            let (size, pixels): ([usize; 2], Vec<u8>) = match &d.image {
                egui::ImageData::Color(img) => (img.size, img.pixels.iter().flat_map(|c| c.to_array()).collect()),
                egui::ImageData::Font(f) => (f.size, f.srgba_pixels(None).flat_map(|c| c.to_array()).collect()),
            };
            unsafe {
                match (d.pos, self.textures.get(id)) {
                    (Some(pos), Some(t)) if pos[0] + size[0] <= t.size[0] && pos[1] + size[1] <= t.size[1] => {
                        let b = D3D11_BOX {
                            left: pos[0] as u32,
                            top: pos[1] as u32,
                            front: 0,
                            right: (pos[0] + size[0]) as u32,
                            bottom: (pos[1] + size[1]) as u32,
                            back: 1,
                        };
                        self.ctx.UpdateSubresource(&t.tex, 0, Some(&b), pixels.as_ptr() as *const _, (size[0] * 4) as u32, 0);
                    }
                    _ => {
                        if let Some(t) = self.make_texture(size, &pixels) {
                            self.textures.insert(*id, t);
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    unsafe fn make_texture(&self, size: [usize; 2], pixels: &[u8]) -> Option<Tex> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size[0].max(1) as u32,
            Height: size[1].max(1) as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: pixels.as_ptr() as *const _, SysMemPitch: (size[0] * 4) as u32, SysMemSlicePitch: 0 };
        let mut tex = None;
        self.device.CreateTexture2D(&desc, Some(&init), Some(&mut tex)).ok()?;
        let tex = tex?;
        let mut srv = None;
        self.device.CreateShaderResourceView(&tex, None, Some(&mut srv)).ok()?;
        Some(Tex { tex, srv: srv?, size })
    }

    unsafe fn ensure_buffer(&self, slot: &mut Option<(ID3D11Buffer, usize)>, bytes: usize, bind: D3D11_BIND_FLAG) -> Option<ID3D11Buffer> {
        if let Some((b, cap)) = slot {
            if *cap >= bytes {
                return Some(b.clone());
            }
        }
        let cap = bytes.next_power_of_two().max(4096);
        let mut buf = None;
        self.device
            .CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: cap as u32,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: bind.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut buf),
            )
            .ok()?;
        let buf = buf?;
        *slot = Some((buf.clone(), cap));
        Some(buf)
    }

    /// Draws one box's frame into its swap chain and shows it.
    pub fn render(&mut self, swapchain: &IDXGISwapChain1, size_px: [u32; 2], ppp: f32, prims: &[ClippedPrimitive]) -> Result<()> {
        unsafe {
            let back: ID3D11Texture2D = swapchain.GetBuffer(0)?;
            self.draw(&back, size_px, ppp, prims)?;
            drop(back);
            swapchain.Present(1, DXGI_PRESENT(0)).ok()
        }
    }

    /// Draws a frame into an off-screen picture and returns it as RGBA rows (opaque), for
    /// `--screenshot`.
    pub fn render_offscreen(&mut self, size_px: [u32; 2], ppp: f32, prims: &[ClippedPrimitive]) -> Result<Vec<u8>> {
        unsafe {
            let fail = || windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL);
            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: size_px[0].max(1),
                Height: size_px[1].max(1),
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                ..Default::default()
            };
            let mut target = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut target))?;
            let target = target.ok_or_else(fail)?;
            self.draw(&target, size_px, ppp, prims)?;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.ok_or_else(fail)?;
            self.ctx.CopyResource(&staging, &target);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let (w, h) = (size_px[0] as usize, size_px[1] as usize);
            let mut out = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                let row = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), w * 4);
                for px in row.chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
            self.ctx.Unmap(&staging, 0);
            Ok(out)
        }
    }

    /// Draws egui's triangles into a texture (a swap chain's back buffer or an off-screen one).
    fn draw(&mut self, target: &ID3D11Texture2D, size_px: [u32; 2], ppp: f32, prims: &[ClippedPrimitive]) -> Result<()> {
        unsafe {
            let mut rtv = None;
            self.device.CreateRenderTargetView(target, None, Some(&mut rtv))?;
            let rtv = rtv.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
            let ctx = &self.ctx;
            ctx.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 0.0]);
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: size_px[0] as f32,
                Height: size_px[1] as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            // Screen size in points (vertices come in points).
            let screen = [size_px[0] as f32 / ppp, size_px[1] as f32 / ppp, 0.0f32, 0.0f32];
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            if ctx.Map(&self.cbuf, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).is_ok() {
                std::ptr::copy_nonoverlapping(screen.as_ptr() as *const u8, m.pData as *mut u8, 16);
                ctx.Unmap(&self.cbuf, 0);
            }
            ctx.IASetInputLayout(&self.layout);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cbuf.clone())]));
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            ctx.OMSetBlendState(&self.blend, Some(&[0.0; 4]), 0xffff_ffff);
            ctx.RSSetState(&self.raster);

            // All meshes into one vertex/index buffer.
            let mut verts: Vec<u8> = Vec::new();
            let mut idx: Vec<u32> = Vec::new();
            let mut draws: Vec<(TextureId, egui::Rect, u32, u32, i32)> = Vec::new();
            for cp in prims {
                if let Primitive::Mesh(mesh) = &cp.primitive {
                    if mesh.indices.is_empty() {
                        continue;
                    }
                    let base = (verts.len() / 20) as i32;
                    for v in &mesh.vertices {
                        verts.extend_from_slice(&v.pos.x.to_le_bytes());
                        verts.extend_from_slice(&v.pos.y.to_le_bytes());
                        verts.extend_from_slice(&v.uv.x.to_le_bytes());
                        verts.extend_from_slice(&v.uv.y.to_le_bytes());
                        verts.extend_from_slice(&v.color.to_array());
                    }
                    let start = idx.len() as u32;
                    idx.extend_from_slice(&mesh.indices);
                    draws.push((mesh.texture_id, cp.clip_rect, start, mesh.indices.len() as u32, base));
                }
            }
            if !draws.is_empty() {
                let mut vslot = self.vbuf.take();
                let mut islot = self.ibuf.take();
                let vb = self.ensure_buffer(&mut vslot, verts.len(), D3D11_BIND_VERTEX_BUFFER);
                let ib = self.ensure_buffer(&mut islot, idx.len() * 4, D3D11_BIND_INDEX_BUFFER);
                self.vbuf = vslot;
                self.ibuf = islot;
                if let (Some(vb), Some(ib)) = (vb, ib) {
                    let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                    if ctx.Map(&vb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).is_ok() {
                        std::ptr::copy_nonoverlapping(verts.as_ptr(), m.pData as *mut u8, verts.len());
                        ctx.Unmap(&vb, 0);
                    }
                    if ctx.Map(&ib, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m)).is_ok() {
                        std::ptr::copy_nonoverlapping(idx.as_ptr() as *const u8, m.pData as *mut u8, idx.len() * 4);
                        ctx.Unmap(&ib, 0);
                    }
                    let stride = 20u32;
                    let offset = 0u32;
                    ctx.IASetVertexBuffers(0, 1, Some(&Some(vb.clone())), Some(&stride), Some(&offset));
                    ctx.IASetIndexBuffer(&ib, DXGI_FORMAT_R32_UINT, 0);
                    for (tex, clip, start, count, base) in draws {
                        let Some(t) = self.textures.get(&tex) else { continue };
                        let r = RECT {
                            left: (clip.min.x * ppp).round().max(0.0) as i32,
                            top: (clip.min.y * ppp).round().max(0.0) as i32,
                            right: (clip.max.x * ppp).round().min(size_px[0] as f32) as i32,
                            bottom: (clip.max.y * ppp).round().min(size_px[1] as f32) as i32,
                        };
                        if r.right <= r.left || r.bottom <= r.top {
                            continue;
                        }
                        ctx.RSSetScissorRects(Some(&[r]));
                        ctx.PSSetShaderResources(0, Some(&[Some(t.srv.clone())]));
                        ctx.DrawIndexed(count, start, base);
                    }
                }
            }
            ctx.OMSetRenderTargets(None, None);
            Ok(())
        }
    }

    pub fn resize(&self, swapchain: &IDXGISwapChain1, w: u32, h: u32) -> Result<()> {
        unsafe { swapchain.ResizeBuffers(2, w.max(1), h.max(1), DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0)) }
    }

    #[allow(dead_code)]
    pub fn device_ok(&self) -> bool {
        let _ = self.device.cast::<IDXGIDevice>().is_ok();
        true
    }
}
