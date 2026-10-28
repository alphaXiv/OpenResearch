import { useEffect, useRef } from "react";

const STAR_COUNT = 600;
const RAMP_MS = 700;
const WARP_MS = 1300;
const SETTLE_MS = 900;
const PEAK_SPEED = 2.6;
const DRIFT_SPEED = 0.06;

type Star = { x: number; y: number; z: number; tinted: boolean };

function speedAt(t: number): number {
  if (t < RAMP_MS) return DRIFT_SPEED + (PEAK_SPEED - DRIFT_SPEED) * (t / RAMP_MS) ** 2;
  if (t < WARP_MS) return PEAK_SPEED;
  const settle = Math.min(1, (t - WARP_MS) / SETTLE_MS);
  return DRIFT_SPEED + (PEAK_SPEED - DRIFT_SPEED) * (1 - settle) ** 3;
}

function spawn(z = Math.random()): Star {
  return { x: Math.random() * 2 - 1, y: Math.random() * 2 - 1, z: Math.max(z, 0.02), tinted: Math.random() < 0.18 };
}

// Light-speed starfield: streaks accelerate out from the centre, then settle into a slow drift.
export function Hyperspace({ className }: { className?: string }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx || matchMedia("(prefers-reduced-motion: reduce)").matches) return;

    const stars = Array.from({ length: STAR_COUNT }, () => spawn());
    let width = 0;
    let height = 0;
    const resize = () => {
      const dpr = window.devicePixelRatio || 1;
      width = canvas.clientWidth;
      height = canvas.clientHeight;
      canvas.width = width * dpr;
      canvas.height = height * dpr;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    resize();
    window.addEventListener("resize", resize);

    const start = performance.now();
    let last = start;
    let frame = 0;
    const draw = (now: number) => {
      // Once settled, drift at ~30fps so an idle welcome screen stays cheap.
      if (now - start > WARP_MS + SETTLE_MS && now - last < 33) {
        frame = requestAnimationFrame(draw);
        return;
      }
      const dt = Math.min(now - last, 50) / 1000;
      last = now;
      const speed = speedAt(now - start);
      // Read per frame so an OS theme flip mid-intro recolours the stars.
      const root = getComputedStyle(document.documentElement);
      const textColor = root.getPropertyValue("--text");
      const primaryColor = root.getPropertyValue("--primary");
      const cx = width / 2;
      const cy = height / 2;
      const focal = Math.max(width, height) * 0.5;
      const tail = Math.min(0.6, speed * 0.12);
      ctx.clearRect(0, 0, width, height);
      ctx.lineCap = "round";
      for (const star of stars) {
        star.z -= speed * dt;
        if (star.z <= 0.02) Object.assign(star, spawn(1));
        const headX = cx + (star.x / star.z) * focal;
        const headY = cy + (star.y / star.z) * focal;
        const tailZ = star.z + tail;
        const tailX = cx + (star.x / tailZ) * focal;
        const tailY = cy + (star.y / tailZ) * focal;
        const depth = 1 - star.z;
        ctx.globalAlpha = Math.min(1, depth * 1.4) * (star.tinted ? 1 : 0.8);
        ctx.strokeStyle = star.tinted ? primaryColor : textColor;
        ctx.lineWidth = 0.6 + depth * 2.2;
        ctx.beginPath();
        ctx.moveTo(tailX, tailY);
        ctx.lineTo(headX + 0.01, headY);
        ctx.stroke();
      }
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", resize);
    };
  }, []);

  return <canvas ref={canvasRef} aria-hidden="true" className={className} />;
}
