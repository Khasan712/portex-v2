from datetime import datetime
import calendar
import os


def get_log_dir(base_dir):
    """
    - Check a subdirectory(according to current year and month) exists or not
    - If it does not exist, create a subdirectory according to current year and month
    - Return the directory path
    """
    try:
        now = datetime.now()
        year = str(now.year)
        month_name = calendar.month_name[now.month]

        # Create the directory path
        # Create a path for the directory in the root directory
        root_directory = os.path.abspath(os.sep)  # Get the root directory path
        directory_path = os.path.join(root_directory, 'logs', year, month_name)  # using root directory
        # directory_path = os.path.join(base_dir, 'logs', year, month_name)  # using base directory

        # Check if the directory exists. If it doesn't exist, create it
        if not os.path.exists(directory_path):
            os.makedirs(directory_path)
            print("Directory created successfully:", directory_path)
        else:
            print("Directory already exists:", directory_path)
        return directory_path

    except Exception as ex:
        print("Cannot create directory for logs. Error => ", ex)
        return base_dir

