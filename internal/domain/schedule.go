package domain

import (
	"fmt"
	"strings"
	"time"

	"github.com/robfig/cron/v3"
)

// ScheduledPrompt starts an independent workspace at a saved local time.
type ScheduledPrompt struct {
	Flags      map[string]string   `json:"flags,omitempty"`
	Simplify   *bool               `json:"simplify,omitempty"`
	Cron       string              `json:"cron"`
	LoopID     string              `json:"loopId"`
	PolicyID   string              `json:"policyId"`
	ID         string              `json:"id"`
	ProjectID  string              `json:"projectId"`
	Name       string              `json:"name"`
	Prompt     string              `json:"prompt"`
	BaseBranch string              `json:"baseBranch"`
	Mode       string              `json:"mode"`
	Model      string              `json:"model"`
	Cadence    string              `json:"cadence"`
	Date       string              `json:"date"`
	Time       string              `json:"time"`
	TimeZone   string              `json:"timeZone"`
	Paused     bool                `json:"paused"`
	NextRunAt  int64               `json:"nextRunAt"`
	LastRun    *ScheduledPromptRun `json:"lastRun,omitempty"`
}

// ScheduledPromptRun is a durable occurrence, claimed before any side effects.
type ScheduledPromptRun struct {
	ID          string `json:"id"`
	ScheduleID  string `json:"scheduleId"`
	DueAt       int64  `json:"dueAt"`
	WorkspaceID string `json:"workspaceId"`
	Status      string `json:"status"`
	Detail      string `json:"detail"`
}

var scheduledCronParser = cron.NewParser(cron.SecondOptional | cron.Minute | cron.Hour | cron.Dom | cron.Month | cron.Dow | cron.Descriptor)

// OnStartup schedules a run when Recur starts, the desktop equivalent of @reboot.
func (s ScheduledPrompt) OnStartup() bool {
	return s.Cadence == "cron" && strings.TrimSpace(s.Cron) == "@reboot"
}

// NextScheduledTime returns the first occurrence strictly after after, or zero
// for an exhausted one-time schedule or @reboot. Cron uses its own calendar and
// DST semantics; legacy daily/weekday schedules retain their original behavior.
func NextScheduledTime(s ScheduledPrompt, after time.Time) (time.Time, error) {
	loc, err := time.LoadLocation(s.TimeZone)
	if err != nil || s.TimeZone == "" || s.TimeZone == "Local" {
		return time.Time{}, fmt.Errorf("choose a valid IANA time zone")
	}
	if s.OnStartup() {
		return time.Time{}, nil
	}
	if s.Cadence == "cron" {
		parsed, err := scheduledCronParser.Parse(strings.TrimSpace(s.Cron))
		if err != nil {
			return time.Time{}, fmt.Errorf("invalid cron expression: %w", err)
		}
		next := parsed.Next(after.In(loc))
		if next.IsZero() {
			return time.Time{}, fmt.Errorf("cron expression has no upcoming occurrence")
		}
		return next, nil
	}
	clock, err := time.Parse("15:04", s.Time)
	if err != nil {
		return time.Time{}, fmt.Errorf("time must be HH:MM")
	}
	day := after.In(loc)
	switch s.Cadence {
	case "once":
		day, err = time.Parse("2006-01-02", s.Date)
		if err != nil {
			return time.Time{}, fmt.Errorf("choose a valid date")
		}
	case "daily", "weekdays":
	default:
		return time.Time{}, fmt.Errorf("choose a one-time run or cron schedule")
	}
	for i := 0; i < 8; i++ {
		y, m, d := day.Date()
		noon := time.Date(y, m, d, 12, 0, 0, 0, loc)
		start, end := noon.Add(-24*time.Hour), noon.Add(24*time.Hour)
		if s.Cadence != "weekdays" || (noon.Weekday() != time.Saturday && noon.Weekday() != time.Sunday) {
			for t := start; t.Before(end); t = t.Add(time.Minute) {
				ty, tm, td := t.Date()
				if ty == y && tm == m && td == d && t.Hour() == clock.Hour() && t.Minute() == clock.Minute() {
					if t.After(after) {
						return t, nil
					}
					break // The first occurrence already passed, including on a fall-back day.
				}
			}
		}
		if s.Cadence == "once" {
			break
		}
		day = time.Date(y, m, d+1, 12, 0, 0, 0, loc)
	}
	return time.Time{}, nil
}
