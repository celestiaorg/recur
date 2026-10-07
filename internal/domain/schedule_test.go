package domain

import (
	"testing"
	"time"
)

func TestNextScheduledTime(t *testing.T) {
	for _, tt := range []struct{ name, cadence, date, clock, zone, after, want string }{
		{"daily", "daily", "", "09:00", "America/New_York", "2026-09-08T12:00:00Z", "2026-09-08T13:00:00Z"},
		{"strictly after", "daily", "", "09:00", "America/New_York", "2026-09-08T13:00:00Z", "2026-09-09T13:00:00Z"},
		{"weekend", "weekdays", "", "09:00", "America/New_York", "2026-09-11T14:00:00Z", "2026-09-14T13:00:00Z"},
		{"midnight gap", "daily", "", "00:00", "America/Havana", "2026-03-08T04:00:00Z", "2026-03-09T04:00:00Z"},
		{"before midnight gap", "daily", "", "23:30", "America/Havana", "2026-03-08T04:00:00Z", "2026-03-08T04:30:00Z"},
		{"spring gap", "daily", "", "02:30", "America/New_York", "2026-03-08T05:00:00Z", "2026-03-09T06:30:00Z"},
		{"fall first", "daily", "", "01:30", "America/New_York", "2026-11-01T04:00:00Z", "2026-11-01T05:30:00Z"},
		{"fall never twice", "daily", "", "01:30", "America/New_York", "2026-11-01T05:30:00Z", "2026-11-02T06:30:00Z"},
		{"half hour transition", "daily", "", "01:45", "Australia/Lord_Howe", "2026-04-04T14:45:00Z", "2026-04-05T15:15:00Z"},
		{"once", "once", "2026-09-09", "09:00", "UTC", "2026-09-08T13:00:00Z", "2026-09-09T09:00:00Z"},
		{"once exhausted", "once", "2026-09-08", "09:00", "UTC", "2026-09-08T13:00:00Z", ""},
		{"once nonexistent", "once", "2026-03-08", "02:30", "America/New_York", "2026-03-07T00:00:00Z", ""},
	} {
		t.Run(tt.name, func(t *testing.T) {
			after, err := time.Parse(time.RFC3339, tt.after)
			if err != nil {
				t.Fatal(err)
			}
			got, err := NextScheduledTime(ScheduledPrompt{Cadence: tt.cadence, Date: tt.date, Time: tt.clock, TimeZone: tt.zone}, after)
			if err != nil {
				t.Fatal(err)
			}
			result := ""
			if !got.IsZero() {
				result = got.UTC().Format(time.RFC3339)
			}
			if result != tt.want {
				t.Errorf("NextScheduledTime after %s = %s, want %s", tt.after, result, tt.want)
			}
		})
	}
}

func TestNextScheduledTimeRejectsInvalidInput(t *testing.T) {
	for _, p := range []ScheduledPrompt{
		{Cadence: "daily", Time: "09:00", TimeZone: "Local"},
		{Cadence: "daily", Time: "09:00", TimeZone: "invalid/zone"},
		{Cadence: "daily", Time: "24:00", TimeZone: "UTC"},
		{Cadence: "cron", Time: "09:00", TimeZone: "UTC"},
		{Cadence: "once", Date: "2026-02-30", Time: "09:00", TimeZone: "UTC"},
	} {
		if _, err := NextScheduledTime(p, time.Now()); err == nil {
			t.Errorf("NextScheduledTime(%+v) accepted invalid input", p)
		}
	}
}

func TestCronScheduledTime(t *testing.T) {
	for _, tt := range []struct{ expression, zone, after, want string }{
		{"*/15 9-17 * * MON-FRI", "UTC", "2026-09-11T17:45:00Z", "2026-09-14T09:00:00Z"},
		{"0 9,17 * * *", "UTC", "2026-09-08T09:00:00Z", "2026-09-08T17:00:00Z"},
		{"0 9 1,15 * *", "UTC", "2026-09-08T09:00:00Z", "2026-09-15T09:00:00Z"},
		{"0 9 * JAN,MAR MON", "UTC", "2026-09-08T09:00:00Z", "2027-01-04T09:00:00Z"},
		{"0 9 15 * MON", "UTC", "2026-09-08T09:00:00Z", "2026-09-14T09:00:00Z"},
		{"0 0 29 FEB *", "UTC", "2026-09-08T09:00:00Z", "2028-02-29T00:00:00Z"},
		{"*/10 * * * * *", "UTC", "2026-09-08T09:00:01Z", "2026-09-08T09:00:10Z"},
		{"@monthly", "UTC", "2026-09-08T09:00:00Z", "2026-10-01T00:00:00Z"},
		{"@every 2h30m", "UTC", "2026-09-08T09:00:00Z", "2026-09-08T11:30:00Z"},
		{"0 9 * * *", "America/New_York", "2026-09-08T09:00:00Z", "2026-09-08T13:00:00Z"},
		{"CRON_TZ=Asia/Tokyo 0 9 * * *", "UTC", "2026-09-08T09:00:00Z", "2026-09-09T00:00:00Z"},
		{"30 2 * * *", "America/New_York", "2026-03-08T05:00:00Z", "2026-03-09T06:30:00Z"},
		{"30 1 * * *", "America/New_York", "2026-11-01T05:30:00Z", "2026-11-01T06:30:00Z"},
	} {
		t.Run(tt.expression+tt.after, func(t *testing.T) {
			after, _ := time.Parse(time.RFC3339, tt.after)
			got, err := NextScheduledTime(ScheduledPrompt{Cadence: "cron", Cron: tt.expression, TimeZone: tt.zone}, after)
			if err != nil || got.UTC().Format(time.RFC3339) != tt.want {
				t.Fatalf("next = %v, %v; want %s", got, err, tt.want)
			}
		})
	}
	for _, expression := range []string{"bad cron", "60 * * * *", "*/0 * * * *", "0 0 30 FEB *", "@every nonsense"} {
		if _, err := NextScheduledTime(ScheduledPrompt{Cadence: "cron", Cron: expression, TimeZone: "UTC"}, time.Now()); err == nil {
			t.Errorf("accepted invalid expression %q", expression)
		}
	}
	if next, err := NextScheduledTime(ScheduledPrompt{Cadence: "cron", Cron: "@reboot", TimeZone: "UTC"}, time.Now()); err != nil || !next.IsZero() {
		t.Fatalf("startup schedule = %v, %v; want no calendar occurrence", next, err)
	}
}
