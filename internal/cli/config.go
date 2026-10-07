package cli

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/viper"

	"recur/internal/store"
)

// initConfig wires Viper for the Recur CLI.
//
// Precedence (high → low): flags (when bound) → env (RECUR_*) → config file
// (~/.recur/config.yaml) → defaults.
func initConfig() error {
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	defaultBundles := filepath.Join(home, ".recur", "bundles")

	viper.SetConfigName("config")
	viper.SetConfigType("yaml")
	viper.AddConfigPath(filepath.Join(home, ".recur"))
	viper.SetEnvPrefix("RECUR")
	viper.SetEnvKeyReplacer(strings.NewReplacer("-", "_", ".", "_"))
	viper.AutomaticEnv()
	viper.SetDefault("bundles-dir", defaultBundles)

	// Missing config file is fine; env + defaults still apply.
	_ = viper.ReadInConfig()
	return nil
}

// bundlesDir returns the user bundles root (Viper bundles-dir / RECUR_BUNDLES_DIR).
func bundlesDir() (string, error) {
	if d := strings.TrimSpace(viper.GetString("bundles-dir")); d != "" {
		if err := os.MkdirAll(d, 0o755); err != nil {
			return "", err
		}
		return d, nil
	}
	return store.UserBundlesDir()
}
