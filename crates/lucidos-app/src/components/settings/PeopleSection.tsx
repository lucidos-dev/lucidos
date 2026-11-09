import { Dropdown } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import {
  addEmailPrompt,
  addOperatorPrompt,
  isTeamGateway,
  moveDevice,
  operatorRows,
  operators,
  removeEmail,
  removeOperatorConfirmed,
  renameOperatorPrompt,
} from '../../store/actions/operators';
import { NO_GATEWAY } from '../../store/actions/pairedDevices';

/** Which person a paired device belongs to, as a dropdown on its Devices row.
 *  Drawn only with several people: with one there is nothing to pick. */
export function DevicePersonPicker({ deviceId }: { deviceId: string }) {
  const people = operatorRows(operators.value);
  if (!isTeamGateway.value || people === null) return null;
  const holder = people.find((o) => o.device_ids.includes(deviceId));
  if (!holder) return null;
  return (
    <Dropdown
      options={people.map((o) => ({ value: o.id, label: o.name }))}
      value={holder.id}
      onChange={(value) => void moveDevice(deviceId, value)}
    />
  );
}

/** People: the operators of this gateway (ADR 0623). A person is a name, the
 *  emails they write from, and the devices they hold. Nobody gets more or
 *  less access by being a person here. */
export function PeopleSection() {
  const list = operators.value;
  if (list.status === 'loaded' && list.data === NO_GATEWAY) return null;
  const people = operatorRows(list);
  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="devices:people">
        People
        <Explainer title="People">
          <p>
            A person is a name, the emails they use elsewhere, and the devices they
            hold. On a shared gateway it decides whose questions and notifications
            are whose, and who changed what. It never limits what a device can do.
          </p>
          <p>
            People belong to this machine, not one workspace, so every workspace here
            sees the same people.
          </p>
        </Explainer>
      </div>
      <div class="list-rows">
        {list.status === 'failed' && <LoadableError noun="people" error={list.error} />}
        {isTeamGateway.value && people?.map((person) => {
          const count = person.device_ids.length;
          return (
            <div class="list-row" key={person.id}>
              <div class="list-row-info">
                <div class="title">{person.name}</div>
                <div class="list-row-details">
                  <span>{count === 1 ? '1 device' : `${count} devices`}</span>
                  {person.emails.map((email) => (
                    <button
                      key={email}
                      class="action-btn"
                      data-tooltip={`Remove ${email}`}
                      onClick={() => void removeEmail(person, email)}
                    >{email} ×</button>
                  ))}
                </div>
              </div>
              <div class="list-row-actions">
                <button class="action-btn" onClick={() => void addEmailPrompt(person)}>Add email</button>
                <button class="action-btn" onClick={() => void renameOperatorPrompt(person)}>Rename</button>
                <button
                  class="action-btn action-btn-danger"
                  data-tooltip="Revoke every device of this person"
                  onClick={() => void removeOperatorConfirmed(person)}
                >Remove</button>
              </div>
            </div>
          );
        })}
        <div class="list-row">
          <div class="list-row-info">
            <div class="list-row-details list-row-details-prose">
              {isTeamGateway.value
                ? 'Add someone, then pair their devices for them under Settings → Access.'
                : 'Sharing this machine with a team? Add each person, so work and notifications can be theirs.'}
            </div>
          </div>
          <div class="list-row-actions">
            <button class="action-btn" onClick={() => void addOperatorPrompt()}>Add a person</button>
          </div>
        </div>
      </div>
    </div>
  );
}
