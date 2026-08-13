const SHADER = /* wgsl */ `
struct Uniforms {
  viewport: vec2<f32>,
  pointer: vec2<f32>,
  time: f32,
  energy: f32,
  radius: f32,
  dark: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOutput {
  @builtin(position) position: vec4<f32>,
}

@vertex
fn vertex_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
  var triangle = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(3.0, -1.0),
    vec2<f32>(-1.0, 3.0),
  );
  var output: VertexOutput;
  output.position = vec4<f32>(triangle[vertex_index], 0.0, 1.0);
  return output;
}

fn rounded_box(point: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
  let q = abs(point) - half_size + vec2<f32>(radius);
  return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn hash(point: vec2<f32>) -> f32 {
  return fract(sin(dot(point, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

@fragment
fn fragment_main(@builtin(position) fragment: vec4<f32>) -> @location(0) vec4<f32> {
  let center = u.viewport * 0.5;
  let point = fragment.xy - center;
  let half_size = max(center - vec2<f32>(1.5), vec2<f32>(1.0));
  let distance = rounded_box(point, half_size, min(u.radius, min(half_size.x, half_size.y)));
  let inside = 1.0 - smoothstep(-0.75, 1.0, distance);
  if (inside <= 0.001) { discard; }

  let normalized = point / max(half_size, vec2<f32>(1.0));
  let pointer = (u.pointer - vec2<f32>(0.5)) * 2.0;
  let rim = exp(-abs(distance) * 0.18);
  let upper = pow(clamp(1.0 - (normalized.y + 1.0) * 0.5, 0.0, 1.0), 5.0);
  let focus = exp(-length(normalized - pointer * vec2<f32>(0.42, 0.7)) * 3.2);
  let wave = 0.5 + 0.5 * sin(normalized.x * 7.0 - normalized.y * 4.0 + u.time * 0.45);
  let grain = hash(floor(fragment.xy * 0.55) + floor(u.time * 3.0)) - 0.5;

  let phase = normalized.x * 1.6 + normalized.y * 0.8 + pointer.x * 0.65;
  let spectrum = vec3<f32>(
    0.58 + 0.42 * sin(phase + 0.0),
    0.62 + 0.38 * sin(phase + 2.1),
    0.70 + 0.30 * sin(phase + 4.2)
  );
  let neutral = mix(vec3<f32>(0.88, 0.94, 1.0), vec3<f32>(0.72, 0.84, 0.95), u.dark);
  let spectral_rim = mix(neutral, spectrum, 0.58) * rim;
  let state_glow = vec3<f32>(0.18, 0.66, 1.0) * rim * u.energy;
  let highlight = neutral * (upper * 0.34 + focus * 0.17 + wave * rim * 0.055);
  let alpha = inside * clamp(rim * (0.12 + u.energy * 0.08) + upper * 0.08 + focus * 0.04 + grain * 0.012, 0.0, 0.34);
  let color = spectral_rim * 0.72 + state_glow * 0.36 + highlight;
  return vec4<f32>(color * alpha, alpha);
}
`;

function fallback(canvas, reason) {
  canvas.dataset.renderer = "css";
  canvas.dataset.rendererReason = reason;
  return { stop() {} };
}

async function start(canvas) {
  if (!navigator.gpu) return fallback(canvas, "webgpu-unavailable");
  if (matchMedia("(prefers-reduced-transparency: reduce)").matches) {
    return fallback(canvas, "reduced-transparency");
  }

  const adapter = await navigator.gpu.requestAdapter({ powerPreference: "low-power" });
  if (!adapter) return fallback(canvas, "adapter-unavailable");
  const device = await adapter.requestDevice();
  const context = canvas.getContext("webgpu");
  if (!context) return fallback(canvas, "context-unavailable");

  const format = navigator.gpu.getPreferredCanvasFormat();
  context.configure({ device, format, alphaMode: "premultiplied" });
  const module = device.createShaderModule({ label: "Stalky glass spectral shader", code: SHADER });
  const pipeline = device.createRenderPipeline({
    label: "Stalky glass pipeline",
    layout: "auto",
    vertex: { module, entryPoint: "vertex_main" },
    fragment: {
      module,
      entryPoint: "fragment_main",
      targets: [{
        format,
        blend: {
          color: { srcFactor: "one", dstFactor: "one-minus-src-alpha" },
          alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha" },
        },
      }],
    },
    primitive: { topology: "triangle-list" },
  });
  const uniformBuffer = device.createBuffer({
    label: "Stalky glass uniforms",
    size: 32,
    usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
  });
  const bindGroup = device.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: [{ binding: 0, resource: { buffer: uniformBuffer } }],
  });

  let stopped = false;
  let frame = 0;
  let pointerX = 0.28;
  let pointerY = 0.18;
  let width = 1;
  let height = 1;
  const root = canvas.closest(".glance-root");
  const darkQuery = matchMedia("(prefers-color-scheme: dark)");
  const motionQuery = matchMedia("(prefers-reduced-motion: reduce)");

  const resize = () => {
    const bounds = canvas.getBoundingClientRect();
    const scale = Math.min(devicePixelRatio || 1, 2);
    width = Math.max(1, Math.round(bounds.width * scale));
    height = Math.max(1, Math.round(bounds.height * scale));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
  };
  const move = (event) => {
    const bounds = canvas.getBoundingClientRect();
    pointerX = Math.max(0, Math.min(1, (event.clientX - bounds.left) / Math.max(bounds.width, 1)));
    pointerY = Math.max(0, Math.min(1, (event.clientY - bounds.top) / Math.max(bounds.height, 1)));
  };
  const resizeObserver = new ResizeObserver(resize);
  resizeObserver.observe(canvas);
  window.addEventListener("pointermove", move, { passive: true });
  resize();

  const startedAt = performance.now();
  const render = (now) => {
    if (stopped) return;
    const running = root?.classList.contains("running") ? 1 : 0;
    const attention = root?.classList.contains("attention") ? 1 : 0;
    const energy = Math.max(running * 0.85, attention);
    const time = motionQuery.matches ? 0 : (now - startedAt) / 1000;
    const scale = Math.min(devicePixelRatio || 1, 2);
    const uniforms = new Float32Array([
      width, height, pointerX, pointerY,
      time, energy, 18 * scale, darkQuery.matches ? 1 : 0,
    ]);
    device.queue.writeBuffer(uniformBuffer, 0, uniforms);

    const encoder = device.createCommandEncoder({ label: "Stalky glass frame" });
    const pass = encoder.beginRenderPass({
      colorAttachments: [{
        view: context.getCurrentTexture().createView(),
        clearValue: { r: 0, g: 0, b: 0, a: 0 },
        loadOp: "clear",
        storeOp: "store",
      }],
    });
    pass.setPipeline(pipeline);
    pass.setBindGroup(0, bindGroup);
    pass.draw(3);
    pass.end();
    device.queue.submit([encoder.finish()]);
    frame = requestAnimationFrame(render);
  };

  canvas.dataset.renderer = "webgpu";
  frame = requestAnimationFrame(render);
  return {
    stop() {
      stopped = true;
      cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      window.removeEventListener("pointermove", move);
      context.unconfigure();
      uniformBuffer.destroy();
      device.destroy();
    },
  };
}

function stop(controller) {
  controller?.stop?.();
}

window.__STALKY_GLASS__ = { start, stop };
