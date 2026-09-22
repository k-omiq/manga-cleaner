import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/svelte'
import CloudCostDialog from './CloudCostDialog.svelte'
import { t } from '../i18n/index.js'

afterEach(cleanup)

describe('CloudCostDialog', () => {
  it('renders formatted currency when estimated cost is a valid positive number', () => {
    const spec = {
      kind: 'cloudCost',
      titleKey: 'modal.title.cloudCost',
      blocking: true,
      dismissable: true,
      props: {
        regionId: 'r1',
        estimatedCost: 0.045,
      },
      actions: [
        { id: 'cancel', labelKey: 'shell.action.cancel' },
        { id: 'confirm', labelKey: 'shell.action.confirmSpend', variant: 'primary' },
      ],
    }

    const { getByText } = render(CloudCostDialog, { spec })
    const rendered = getByText(t('modal.body.cloudCost', { cost: 0.045 }))
    expect(rendered).toBeTruthy()
    expect(rendered.textContent).toContain('$0.045')
    expect(rendered.textContent).not.toContain('billed on return')
    expect(rendered.textContent).not.toContain('not billed')
  })

  it('renders formatted zero currency when estimated cost is exactly zero', () => {
    const spec = {
      kind: 'cloudCost',
      titleKey: 'modal.title.cloudCost',
      blocking: true,
      dismissable: true,
      props: {
        regionId: 'r1',
        estimatedCost: 0,
      },
      actions: [
        { id: 'cancel', labelKey: 'shell.action.cancel' },
        { id: 'confirm', labelKey: 'shell.action.confirmSpend', variant: 'primary' },
      ],
    }

    const { getByText } = render(CloudCostDialog, { spec })
    const rendered = getByText(t('modal.body.cloudCost', { cost: 0 }))
    expect(rendered).toBeTruthy()
    expect(rendered.textContent).toContain('$0.00')
    expect(rendered.textContent).not.toContain('Cost estimate unavailable')
  })

  it('renders unknown cost statement without fabricating $0 when estimatedCost is null', () => {
    const spec = {
      kind: 'cloudCost',
      titleKey: 'modal.title.cloudCost',
      blocking: true,
      dismissable: true,
      props: {
        regionId: 'r1',
        estimatedCost: null,
      },
      actions: [
        { id: 'cancel', labelKey: 'shell.action.cancel' },
        { id: 'confirm', labelKey: 'shell.action.confirmSpend', variant: 'primary' },
      ],
    }

    const { getByText, queryByText } = render(CloudCostDialog, { spec })
    const rendered = getByText(t('modal.body.cloudCostUnknown'))
    expect(rendered).toBeTruthy()
    expect(rendered.textContent).toContain('Cost estimate unavailable')
    expect(rendered.textContent).not.toContain('billed on return')
    expect(queryByText('$0.00')).toBeNull()
    expect(queryByText('Estimated $0.00')).toBeNull()
  })

  it('renders unknown cost statement when estimatedCost is missing or undefined', () => {
    const spec = {
      kind: 'cloudCost',
      titleKey: 'modal.title.cloudCost',
      blocking: true,
      dismissable: true,
      props: {
        regionId: 'r1',
      },
      actions: [
        { id: 'cancel', labelKey: 'shell.action.cancel' },
        { id: 'confirm', labelKey: 'shell.action.confirmSpend', variant: 'primary' },
      ],
    }

    const { getByText, queryByText } = render(CloudCostDialog, { spec })
    const rendered = getByText(t('modal.body.cloudCostUnknown'))
    expect(rendered).toBeTruthy()
    expect(rendered.textContent).toContain('Cost estimate unavailable')
    expect(queryByText('$0.00')).toBeNull()
  })
})
