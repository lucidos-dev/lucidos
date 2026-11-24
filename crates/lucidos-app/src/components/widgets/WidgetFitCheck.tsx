import { useEffect, useRef, useState } from 'preact/hooks';
import { SETTLE_CAP_MS } from '@lucidos/widget-frame';
import { postWidgetFitReportApi, type WidgetFitBox, type WidgetFitSurface } from '../../api/client/widgets';
import { WIDGET_FIT_SCALE_ATTRIBUTE } from '../../store/actions/app-appearance';
import type { WidgetFit } from '../../store/actions/widget-frame-bridge';
import { finishWidgetFit, widgetFitRequests, type WidgetFitRequest } from '../../store/widgetFit';
import type { WidgetSize } from '../../store/types';
import { DASHBOARD_CELL_REM, DASHBOARD_GAP_REM, widgetBoxCss, widgetCellPx } from '../../utils/dashboardGeometry';
import { WidgetBox } from './WidgetBox';

export const PHONE_COLUMN_PX = 360;

export const WIDGET_FIT_SURFACES: readonly WidgetFitSurface[] = ['desk-tile', 'phone-column'];

export const FIT_ANSWER_SLACK_MS = 2000;

const NO_ANSWER = 'the widget did not answer: it may not load the SDK, or it failed to load';

function remPxAt(scale: number): number {
  const root = document.documentElement;
  const shell = parseFloat(root.style.getPropertyValue('--user-ui-scale')) || 100;
  return (parseFloat(getComputedStyle(root).fontSize) / shell) * scale;
}

export function surfaceCellPx(surface: WidgetFitSurface, scale: number): { cell: number; gap: number } {
  const rem = remPxAt(scale);
  const cell = surface === 'desk-tile' ? DASHBOARD_CELL_REM * rem : widgetCellPx(PHONE_COLUMN_PX, rem);
  return { cell, gap: DASHBOARD_GAP_REM * rem };
}

function boxOf(surface: WidgetFitSurface, size: WidgetSize, fit: WidgetFit): WidgetFitBox {
  return 'error' in fit
    ? { surface, size, error: fit.error }
    : { surface, size, box: fit.box, findings: fit.findings, total: fit.total };
}

function SurfaceProbe({ surface, request, onDone }: {
  surface: WidgetFitSurface;
  request: WidgetFitRequest;
  onDone: (boxes: WidgetFitBox[]) => void;
}) {
  const [step, setStep] = useState(0);
  const boxes = useRef<WidgetFitBox[]>([]);
  const done = useRef(false);
  const size = request.sizes[step];
  const measure = `${request.requestId}:${surface}:${size}`;

  const finish = () => {
    done.current = true;
    onDone(boxes.current);
  };
  const record = (box: WidgetFitBox) => {
    if (done.current) return;
    boxes.current.push(box);
    if (step + 1 < request.sizes.length) setStep(step + 1);
    else finish();
  };

  useEffect(() => {
    if (size === undefined) return undefined;
    const deadline = setTimeout(() => {
      if (done.current) return;
      boxes.current.push(...request.sizes.slice(step).map((s) => ({ surface, size: s, error: NO_ANSWER })));
      finish();
    }, SETTLE_CAP_MS + FIT_ANSWER_SLACK_MS);
    return () => clearTimeout(deadline);
  }, [measure]);

  if (size === undefined) return null;
  const { cell, gap } = surfaceCellPx(surface, request.scale);
  const box = widgetBoxCss(size);
  return (
    <div class="widget-fit-surface" data-surface={surface} style={{ '--widget-cell': `${cell}px`, '--widget-gap': `${gap}px` }}>
      <WidgetBox
        appId={request.appId}
        reveal="on-load"
        place="dashboard"
        size={size}
        style={{ width: box.width, height: box.height }}
        probe={{ measure, device: request.scaleDevice ?? null, onFit: (fit) => record(boxOf(surface, size, fit)) }}
      />
    </div>
  );
}

function FitRun({ request }: { request: WidgetFitRequest }) {
  const results = useRef(new Map<WidgetFitSurface, WidgetFitBox[]>());
  const [finished, setFinished] = useState<WidgetFitSurface[]>([]);

  useEffect(() => {
    if (finished.length < WIDGET_FIT_SURFACES.length) return;
    const boxes = WIDGET_FIT_SURFACES.flatMap((surface) => results.current.get(surface) ?? []);
    postWidgetFitReportApi(request.requestId, boxes)
      .catch((e) => console.warn('[WidgetFit] the fit report did not reach the engine:', e))
      .finally(() => finishWidgetFit(request.requestId));
  }, [finished.length]);

  return (
    <div class="widget-fit-probe" {...{ [WIDGET_FIT_SCALE_ATTRIBUTE]: String(request.scale) }} aria-hidden="true" inert>
      {WIDGET_FIT_SURFACES.map((surface) => !finished.includes(surface) && (
        <SurfaceProbe
          key={surface}
          surface={surface}
          request={request}
          onDone={(boxes) => {
            results.current.set(surface, boxes);
            setFinished((done) => [...done, surface]);
          }}
        />
      ))}
    </div>
  );
}

export function WidgetFitCheck() {
  const request = widgetFitRequests.value[0];
  return request ? <FitRun key={request.requestId} request={request} /> : null;
}
