import { SkBlock, SkText, SkeletonProvider } from './Skeleton';

export interface FormSkeletonField {
  label?: string;
  tall?: boolean;
}

const CONTROL_H = '2.375rem';
const TALL_CONTROL_H = '12rem';

export function FormSkeleton({ fields }: { fields: FormSkeletonField[] }) {
  const groups = fields.map((f, i) => (
    <div class="form-group" key={i}>
      <label>{f.label ?? <SkText w="6rem" />}</label>
      <SkBlock w="100%" h={f.tall ? TALL_CONTROL_H : CONTROL_H} round />
    </div>
  ));
  return (
    <SkeletonProvider>
      <div class="inline-form" aria-hidden="true">{groups}</div>
    </SkeletonProvider>
  );
}
