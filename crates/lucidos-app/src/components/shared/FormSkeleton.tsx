import { SkBlock, SkText, SkeletonProvider } from './Skeleton';

export interface FormSkeletonField {
  /** The field's label, when it is known before the read lands. Without one
   *  the label shimmers too. */
  label?: string;
  /** A multi-line control, such as a code or intent textarea. */
  tall?: boolean;
}

/** One-line control height: `.form-group input`'s padding plus a line. */
const CONTROL_H = '2.375rem';
const TALL_CONTROL_H = '12rem';

/** Fields drawn as a loading placeholder in the form's own `.form-group` boxes:
 *  the label as real text where it is known, and the control's box shimmering.
 *  `inline` wraps them in the `.inline-form` a whole form stands in. */
export function FormSkeleton({ fields, inline = true }: { fields: FormSkeletonField[]; inline?: boolean }) {
  const groups = fields.map((f, i) => (
    <div class="form-group" key={i}>
      <label>{f.label ?? <SkText w="6rem" />}</label>
      <SkBlock w="100%" h={f.tall ? TALL_CONTROL_H : CONTROL_H} round />
    </div>
  ));
  return (
    <SkeletonProvider>
      {inline ? <div class="inline-form" aria-hidden="true">{groups}</div> : <div aria-hidden="true">{groups}</div>}
    </SkeletonProvider>
  );
}
